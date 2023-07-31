use std::{
    collections::{HashMap, HashSet},
    fmt::format,
};

use crate::{
    ast::{BuiltinType, Param, Type},
    ir::*,
    mc::*,
    mc_inst::{
        self, AsmInst, AsmInstTrait, BinOpInst, BinaryOp, BrInst, CMPInst, CallInst, Cond,
        ConstraintsTrait, FBinOpInst, FCMPInst, MovInst, MovType, PrologueInst, RetInst,
        StackOpInstTrait, VCVTInst, VCVTType, VLDRInst, VMovInst, VMovType, VSTRInst,
    },
};

pub fn build(module: &mut Module) -> AsmModule {
    let mut builder = McBuilder::new(module);
    builder.build_module();
    builder.module
}

struct McBuilder<'a> {
    ir_module: &'a Module,
    module: AsmModule,
    // ir func -> call conv
    vfp_callconv_map: HashMap<ValueId, VfpCallConv>,
    // ir bb -> asm bb
    bb_map: HashMap<ValueId, AsmValueId>,
    // ir func -> asm func
    func_map: HashMap<ValueId, AsmValueId>,
    // ir gv -> asm gv
    gv_map: HashMap<ValueId, AsmValueId>,
    // ir value -> vreg
    vreg_map: HashMap<ValueId, VirtReg>,

    vreg_idx: i32,
}

impl Into<AsmGlobalVariable> for GlobalVariableValue {
    fn into(self) -> AsmGlobalVariable {
        todo!()
    }
}

impl Into<AsmFunction> for FunctionValue {
    fn into(self) -> AsmFunction {
        todo!()
    }
}

impl Into<AsmBlock> for BasicBlockValue {
    fn into(self) -> AsmBlock {
        todo!()
    }
}

impl From<ConstValue> for Imm {
    fn from(value: ConstValue) -> Self {
        match value {
            ConstValue::Int(i) => Imm::Int(IntImm {
                value: i.value as u32,
            }),
            ConstValue::Float(f) => Imm::Float(FloatImm {
                value: f.value as f32,
                is_float: true,
            }),
            _ => unimplemented!(),
        }
    }
}

impl From<Type> for AsmTypeTag {
    fn from(ty: Type) -> Self {
        match ty {
            Type::Builtin(b) => match b {
                crate::ast::BuiltinType::Void => AsmTypeTag::VOID,
                crate::ast::BuiltinType::Bool => AsmTypeTag::BOOL,
                crate::ast::BuiltinType::UChar => AsmTypeTag::CHAR,
                crate::ast::BuiltinType::Char => AsmTypeTag::CHAR,
                crate::ast::BuiltinType::UShort => todo!(),
                crate::ast::BuiltinType::Short => todo!(),
                crate::ast::BuiltinType::UInt => AsmTypeTag::INT32,
                crate::ast::BuiltinType::Int => AsmTypeTag::INT32,
                crate::ast::BuiltinType::UInt64 => todo!(),
                crate::ast::BuiltinType::Int64 => todo!(),
                crate::ast::BuiltinType::Float => AsmTypeTag::FLOAT,
                crate::ast::BuiltinType::Double => AsmTypeTag::DOUBLE,
            },
            Type::Pointer(_) => todo!(),
            Type::Array(_) => todo!(),
            Type::Record(_) => todo!(),
            Type::Function(_) => todo!(),
        }
    }
}

impl McBuilder<'_> {
    fn new<'a>(ir_module: &'a Module) -> McBuilder<'a> {
        McBuilder {
            ir_module,
            module: AsmModule::new(),
            vfp_callconv_map: HashMap::new(),
            bb_map: HashMap::new(),
            func_map: HashMap::new(),
            gv_map: HashMap::new(),
            vreg_map: HashMap::new(),
            vreg_idx: 0,
        }
    }

    fn build_module(&mut self) {
        self.build_global_variables();
        self.build_functions();
    }

    fn build_global_variables(&mut self) {
        for (name, id) in &self.ir_module.global_variables.clone() {
            let global = self.ir_module.get_global_var(*id);
            let val = AsmGlobalVariable::from(global.clone().into());
            let val_id = self.module.alloc_value(AsmValue::GlobalVariable(val));
            if global.initializer.is_some() {
                self.module.bss_globals.push(val_id);
            } else {
                self.module.globals.push(val_id);
            }
        }
    }

    fn build_functions(&mut self) {
        for (_, func_id) in &self.ir_module.functions {
            self.build_function(func_id);
        }
    }

    fn build_function(&mut self, func_id: &ValueId) {
        let ssa_func = self.ir_module.get_func(*func_id);
        let asm_func = AsmFunction::from(ssa_func.clone().into());
        let asm_func_id = self.module.alloc_value(AsmValue::Function(asm_func));
        self.module.functions.push(asm_func_id);

        for (name, block_id) in &ssa_func.bbs.bbs.clone() {
            let block = self.ir_module.get_bb(*block_id);
            let asm_block = AsmBlock::from(block.clone().into());
            let asm_block_id = self.module.alloc_value(AsmValue::Block(asm_block));
            self.module.cur_func_mut().bbs.push(asm_block_id);

            self.bb_map.insert(block_id.clone(), asm_block_id.clone());
        }
        // prologue

        let mut prologue = PrologueInst::new(asm_func_id.clone());
        let prologue_id = self
            .module
            .alloc_value(AsmValue::Inst(AsmInst::Prologue(prologue)));
        let asm_func = self.module.get_func_mut(asm_func_id);
        let entry_id = asm_func.entry;
        self.module
            .get_bb_mut(entry_id)
            .insts
            .insert(0, prologue_id);
        // handle callling convention

        let cc = self.get_cc(func_id);
        let nargs = ssa_func.params.len();
        // 把在寄存器里的参数也预先分配VReg。对于内存中的参数由getVReg生成load指令
        for i in 0..nargs {
            let pv = ssa_func.params[i];
            let loc = &cc.as_vfp_call_conv().self_args[i];
            let mut vreg: VirtReg;

            match loc {
                AsmOperand::IntReg(_) => {
                    vreg = self.get_vreg(false);
                    // prologue.set_constraint(&vreg, loc);
                    prologue.set_out_constraint(vreg, loc.clone());
                    self.vreg_map.insert(pv, vreg);
                    prologue.get_defs_mut().push(AsmOperand::VirtReg(vreg));
                }
                AsmOperand::VfpReg(_) => {
                    vreg = self.get_vreg(true);
                    prologue.set_out_constraint(vreg, loc.clone());
                    self.vreg_map.insert(pv, vreg);
                    prologue.get_defs_mut().push(AsmOperand::VirtReg(vreg));
                }
                _ => {
                    // Reserved for convertValue processing
                    assert!(loc.is_stack_operand());
                }
            }
        }

        for (name, block_id) in &ssa_func.bbs.bbs.clone() {
            let bb = self.ir_module.get_bb(*block_id);
            self.build_block(asm_func_id, *block_id, self.bb_map[block_id]);
        }

        // Process phi instructions after the predecessor-successor relationship of the basic block is built
        for (name, block_id) in &ssa_func.bbs.bbs.clone() {
            if self.ir_module.bb_has_phi(*block_id) {
                let bb = self.ir_module.get_bb(*block_id);
                // Phi instructions need to be processed in batches
                let asm_bb_id = self.bb_map[block_id];
                self.visit_phis(self.ir_module.get_phis(*block_id), asm_func_id, asm_bb_id);
            }
        }
    }

    fn build_block(&mut self, asm_func_id: AsmValueId, ssa_bb_id: ValueId, asm_bb_id: AsmValueId) {
        let ssa_bb = self.ir_module.get_bb(ssa_bb_id);
        for inst_id in &ssa_bb.insts {
            let inst_value = self.ir_module.get_inst(*inst_id);
            if inst_value.is_phi() {
                continue;
            }
            if inst_value.is_term() {
                self.visit_term_inst(asm_func_id, *inst_id, asm_bb_id)
            }
            self.visit_non_term_inst(asm_func_id, *inst_id, asm_bb_id)
        }
    }

    fn visit_phis(
        &mut self,
        phi_ids: Vec<ValueId>,
        asm_func_id: AsmValueId,
        asm_bb_id: AsmValueId,
    ) {
        let abb = self.module.get_bb_mut(asm_bb_id);
        let preds = abb.preds;
        assert!(!preds.is_empty());
        if preds.len() == 1 {
            let mut parallel_movs = Vec::new();
            for phi_id in phi_ids {
                let mut target = self.convert_value(phi_id, asm_func_id, asm_bb_id);
                let phi = self.ir_module.get_inst(phi_id).as_phi();
                assert!(phi.incomings.len() == 1);
                for (val_id, _) in &phi.incomings {
                    let mut from = self.convert_value(*val_id, asm_func_id, asm_bb_id);
                    parallel_movs.push((target, from));
                }
            }
            self.make_parallel_movs(asm_bb_id, &parallel_movs);
        } else {
            let size = preds.len();
            for pred_id in preds {
                let pred = self.module.get_bb_mut(pred_id);
                if pred.succs.len() != 1 {
                    panic!(
                        "Unsplit critical edge: {} to {}.",
                        pred_id.index(),
                        asm_bb_id.index()
                    );
                }

                let mut parallel_movs = Vec::new();
                for phi_id in phi_ids {
                    let mut target = self.convert_value(phi_id, asm_func_id, asm_bb_id);
                    let phi = self.ir_module.get_inst(phi_id).as_phi();
                    assert!(size == phi.incomings.len());
                    let mut found = false;
                    for (val_id, from_bb_id) in &phi.incomings {
                        if self.bb_map.get(&from_bb_id) != Some(&pred_id) {
                            continue;
                        }
                        assert!(!found);
                        found = true;
                        let mut from = self.convert_value_with_before_jump(
                            *val_id,
                            asm_func_id,
                            asm_bb_id,
                            true,
                        );
                        parallel_movs.push((target, from));
                    }
                    assert!(found);
                }
                self.make_parallel_movs(pred_id, &parallel_movs);
            }
        }
    }

    fn make_parallel_movs(
        &mut self,
        asm_bb_id: AsmValueId,
        parallel_movs: &Vec<(AsmOperand, AsmOperand)>,
    ) {
        let mut killed = HashSet::new();
        let mut to_add = Vec::new();
        for (key, value) in parallel_movs {
            let is_float = key.is_float();
            if killed.contains(value) {
                let temp = self.get_vreg(is_float);

                let backup = if is_float {
                    VMovInst::new(VMovType::CPY, temp, value.clone())
                } else {
                    MovInst::new(MovType::REG, temp, value.clone(), None)
                };
                to_add.splice(0..0, self.expand_inst_imm(backup));
                let mov = if is_float {
                    VMovInst::new(VMovType::CPY, key.clone(), temp)
                } else {
                    MovInst::new(MovType::REG, key.clone(), temp, None)
                };
                to_add.extend(self.expand_inst_imm(mov));
                killed.insert(key.clone());
            } else {
                let mov = if is_float {
                    VMovInst::new(VMovType::CPY, key.clone(), value.clone())
                } else {
                    MovInst::new(MovType::REG, key.clone(), value.clone(), None)
                };
                to_add.extend(self.expand_inst_imm(mov));
                killed.insert(key.clone());
            }
        }
        self.module.add_all_before_branch(asm_bb_id, to_add)
    }

    fn visit_term_inst(
        &mut self,
        asm_func_id: AsmValueId,
        inst_id: ValueId,
        asm_bb_id: AsmValueId,
    ) {
        let inst_ = self.ir_module.get_inst(inst_id);
        match inst_ {
            InstValue::Jump(jump_inst) => {
                let target_bb_id = jump_inst.bb;
                let target_asm_bb_id = self.bb_map.get(&target_bb_id).unwrap();
                let jmp_inst = BrInst::new(mc_inst::Cond::AL, target_asm_bb_id.clone());
                let mut abb = self.module.get_bb_mut(asm_bb_id);
                abb.insts.push(jmp_inst);

                // 前驱后继维护
                abb.succs = Some(vec![target_asm_bb_id.clone()]);
                let mut target_bb = self.module.get_bb_mut(target_asm_bb_id.clone());
                target_bb.preds.push(asm_bb_id);
            }
            InstValue::Return(ret_inst) => {
                let mut ssa_bb = self.ir_module.get_parent(inst_id);
                let prev_idx = ssa_bb.insts.iter().position(|&x| x == inst_id).unwrap() as i32 - 1;
                let prev = if prev_idx >= 0 {
                    let inst_1 = self.ir_module.get_inst(ssa_bb.insts[prev_idx as usize]);
                    if let InstValue::Call(call_inst) = inst_1 {
                        if call_inst.must_tail {
                            Some(call_inst)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                if prev.is_some() {
                    return;
                }
                let ret_inst = RetInst::new(asm_func_id);
                let mut abb = self.module.get_bb_mut(asm_bb_id);
                if ret_inst.value.is_some() {
                    let cc = self.get_cc(self.func_map.get(&asm_func_id).unwrap());
                    let mut op =
                        self.convert_value(ret_inst.value.unwrap(), asm_func_id, asm_bb_id);
                    self.expand_imm(&mut op, &mut ret_inst.get_uses_mut(), &mut abb.insts);
                    let op = ret_inst.get_uses().first().unwrap();
                    assert!(op.as_virt_reg().is_some());
                    if let AsmOperand::VfpReg(vfp_reg) = &cc.as_vfp_call_conv().ret_reg {
                        ret_inst.set_in_constraint(op.as_virt_reg().unwrap(), vfp_reg.clone());
                    } else if let AsmOperand::Reg(reg) = &cc.as_vfp_call_conv().ret_reg {
                        ret_inst.set_in_constraint(op.as_virt_reg().unwrap(), reg.clone());
                    } else {
                        panic!("Unsupported operation");
                    }
                }
                let ret_inst_id = self
                    .module
                    .alloc_value(AsmValue::Inst(AsmInst::RetInst(ret_inst)));
                abb.insts.push(ret_inst_id);
                abb.succ = Some(vec![]);
            }
            InstValue::Branch(_) => {
                let inst = inst_.downcast_ref::<BranchInst>().unwrap();
                let cond = self.convert_value(&inst.get_operand0(), asm_func_id, asm_bb_id);
                let tb = self.bb_map.get(&inst.get_operand1().b).unwrap();
                let fb = self.bb_map.get(&inst.get_operand2().b).unwrap();
                let mut abb = self.module.get_bb_mut(asm_bb_id);
                abb.insts
                    .extend(self.expand_cmp_imm(CMPInst::new(cond, IntImm::new(0))));
                if tb == abb.next.as_ref() {
                    //  ==0 跳转到false
                    // abb.insts.push(
                    //     BrInst::builder(fb)
                    //         .with_cond(Cond::EQ)
                    //         .with_comment(inst.to_string())
                    //         .build(),
                    // );
                    let br_inst = BrInst::new(Cond::EQ, fb.clone());
                    let br_inst_id = self
                        .module
                        .alloc_value(AsmValue::Inst(AsmInst::Br(br_inst)));
                    abb.insts.push(br_inst_id);
                } else if fb == abb.next.as_ref() {
                    // != 0跳转到true
                    // abb.insts.push(
                    //     BrInst::builder(tb)
                    //         .with_cond(Cond::NE)
                    //         .with_comment(inst.to_string())
                    //         .build(),
                    // );
                    let br_inst = BrInst::new(Cond::NE, tb.clone());
                    let br_inst_id = self
                        .module
                        .alloc_value(AsmValue::Inst(AsmInst::Br(br_inst)));
                    abb.insts.push(br_inst_id);
                } else {
                    // abb.insts.push(
                    //     BrInst::builder(tb)
                    //         .with_cond(Cond::NE)
                    //         .with_comment(inst.to_string())
                    //         .build(),
                    // );
                    // abb.insts.push(BrInst::builder(fb).build());
                    let br_inst = BrInst::new(Cond::NE, tb.clone());
                    let br_inst_id = self
                        .module
                        .alloc_value(AsmValue::Inst(AsmInst::Br(br_inst)));
                    abb.insts.push(br_inst_id);
                }
                abb.succs = Some(vec![tb.clone(), fb.clone()]);
            }
            _ => panic!("Unknown Terminator Inst."),
        }
    }

    fn visit_non_term_inst(
        &mut self,
        asm_func_id: AsmValueId,
        inst_id: ValueId,
        asm_bb_id: AsmValueId,
    ) {
        let func = self.module.get_func(asm_func_id);
        let inst = self.ir_module.get_inst(inst_id);
        let abb = self.module.get_bb_mut(asm_bb_id);
        match inst {
            InstValue::Alloca(alloca) => {
                let offset = func.stack_state.alloc_local(alloca.ty.size() as i64);
                let bin = BinOpInst::new(
                    BinaryOp::Sub,
                    self.convert_value(inst_id, asm_func_id, asm_bb_id),
                    AsmOperand::IntReg(IntReg {
                        ty: RegType::Fp,
                        is_float: false,
                    }),
                    // NumImm::new(offset as i32),
                    AsmOperand::Imm(Imm::Int(IntImm::from(offset as i32))),
                );
                let bin_id = self.module.alloc_value(AsmValue::Inst(AsmInst::BinOp(bin)));
                abb.insts.append(&mut self.expand_bin_op(bin_id));
            }

            InstValue::InfixOp(infix_op) => {
                let op1 = self.convert_value(infix_op.lhs, asm_func_id, asm_bb_id);
                let op2 = self.convert_value(infix_op.rhs, asm_func_id, asm_bb_id);

                if infix_op.op.is_commutative() {
                    if op2.as_imm().is_none() && op1.as_imm().is_some() {
                        std::mem::swap(&mut op1, &mut op2);
                    }
                }

                let to = self.convert_value(inst_id, asm_func_id, asm_bb_id);
                assert_eq!(op1.is_float(), op2.is_float());
                let is_float = op1.is_float();

                if !infix_op.op.is_boolean() {
                    assert_eq!(op1.is_float(), to.is_float());
                    let bin_id = if is_float {
                        let bin = FBinOpInst::new(infix_op.op.into(), to, op1, op2);
                        self.module
                            .alloc_value(AsmValue::Inst(AsmInst::FBinOp(bin)))
                    } else {
                        let bin = BinOpInst::new(infix_op.op.into(), to, op1, op2);
                        self.module.alloc_value(AsmValue::Inst(AsmInst::BinOp(bin)))
                    };
                    abb.insts.append(&mut self.expand_bin_op(bin_id));
                } else {
                    // Generate CMP + conditional MOV
                    let cmp = if is_float {
                        let cmp = FCMPInst::new(op1, op2);
                        self.module.alloc_value(AsmValue::Inst(AsmInst::FCMP(cmp)))
                    } else {
                        let cmp = CMPInst::new(op1, op2);
                        self.module.alloc_value(AsmValue::Inst(AsmInst::CMP(cmp)))
                    };
                    abb.insts.append(&mut self.expand_cmp_imm(cmp));

                    let dest = self.convert_value(inst_id, asm_func_id, asm_bb_id);
                    assert!(!dest.is_float());

                    abb.insts
                        .append(&mut self.module.load_imm(dest, &Imm::Int(IntImm::from(0))));
                    let mov = MovInst::new(
                        MovType::Movw,
                        dest,
                        AsmOperand::Imm(Imm::Int(IntImm::from(1))),
                        Some(infix_op.op.into()),
                    );
                    let mov_id = self.module.alloc_value(AsmValue::Inst(AsmInst::Mov(mov)));
                    abb.insts.push(mov_id);
                }
            }

            InstValue::Call(call) => {
                let ssa_func_id = call.func;
                let ssa_func = self.ir_module.get_func(ssa_func_id);
                let mut call_inst;

                if !ssa_func.is_variadic {
                    let cc = self.get_cc(&ssa_func_id);
                    if call.must_tail {
                        call_inst = mc_inst::TailCallInst::new(
                            LabelImm::new(format!(".LBB_{}_tail_call", ssa_func.name)),
                            cc,
                        );
                    } else {
                        call_inst = mc_inst::CallInst::new(LabelImm::new(ssa_func.name), cc);
                    }

                    for i in 0..ssa_func.params.len() {
                        let loc = if call.must_tail {
                            cc.as_vfp_call_conv().self_args[i]
                        } else {
                            cc.as_vfp_call_conv().call_params[i]
                        };

                        let op = self.convert_value(call.args[i], asm_func_id, asm_bb_id);
                        self.process_call_arg(call_inst, op, loc, asm_bb_id, false);
                    }
                } else {
                    let param_tys = ssa_func
                        .params
                        .iter()
                        .map(|var_id| {
                            let var_val = self.ir_module.get_value(*var_id);
                            let ty = var_val.as_variable().unwrap().ty;
                            ty
                        })
                        .collect();
                    let cc = BaseCallConv::new().resolve(&param_tys, &ssa_func.ret_ty.into());

                    for i in 0..call.args.len() {
                        let arg_val = self.ir_module.get_value(call.args[i]);
                        let mut is_lift_double = false;

                        if i >= ssa_func.params.len() {
                            // variadic
                            cc.add_param(arg_val.ty());
                            if arg_val.ty() == BuiltinType::Double.into() {
                                is_lift_double = true;
                            }
                        }

                        let loc = cc.call_params[i];
                        let op = self.convert_value(arg_val, asm_func_id, asm_bb_id);
                        self.process_call_arg(&mut call_inst, op, loc, asm_bb_id, is_lift_double);
                    }
                }

                if let Some(ret) = call_inst.cc.get_ret_reg() {
                    let ret_val = self.convert_value(call, asm_func_id, asm_bb_id);
                    call_inst.defs.insert(ret_val);
                    if let AsmOperand::IntReg(reg) = ret {
                        call_inst.set_out_constraint(ret_val, reg);
                    } else if let AsmOperand::VfpReg(reg) = ret {
                        call_inst.set_out_constraint(ret_val, reg);
                    } else {
                        unimplemented!();
                    }
                }

                func.sm.preserve_arg_size(call_inst.cc.stack_size());

                abb.insts.push(call_inst);
            }

            InstValue::Cast(cast) => {
                match cast.op {
                    CastOp::Type => {
                        // No-op casts like string -> i8*
                        self.vreg_map.insert(
                            cast,
                            self.convert_value(cast.oprands[0].value, asm_func_id, asm_bb_id),
                        );
                    }
                    CastOp::F2I => {
                        let op = self.convert_value(cast.oprands[0].value, asm_func_id, asm_bb_id);
                        assert!(op.is_float());
                        let mid = self.get_vreg(true);
                        let to = self.convert_value(cast, asm_func_id, asm_bb_id);
                        assert!(!to.is_float());

                        let vcvt = VCVTInst::new(VCVTType::F2I, mid, op);
                        abb.insts.append(&mut self.expand_inst_imm(vcvt));

                        let vmov = VMovInst::new(VMovType::S2A, to, mid);
                        abb.insts.push(vmov);
                    }
                    CastOp::I2F => {
                        let op = self.convert_value(cast.oprands[0].value, asm_func_id, asm_bb_id);
                        assert!(!op.is_float());
                        let mid = self.get_vreg(true);
                        let to = self.convert_value(cast, asm_func_id, asm_bb_id);
                        assert!(to.is_float());

                        let vmov = VMovInst::new(VMovType::A2S, mid, op);
                        abb.insts.append(&mut self.expand_inst_imm(vmov));

                        let vcvt = VCVTInst::new(VCVTType::I2F, to, mid);
                        abb.insts.push(vcvt);
                    }
                    CastOp::FPExt => {
                        // Ignore float -> double promotion for variadic args
                        self.vreg_map.insert(
                            cast,
                            self.convert_value(cast.oprands[0].value, asm_func_id, asm_bb_id),
                        );
                    }
                    CastOp::ZExt => {
                        // i1 -> i32 extension, no-op
                        self.vreg_map.insert(
                            cast,
                            self.convert_value(cast.oprands[0].value, asm_func_id, asm_bb_id),
                        );
                    }
                    _ => unimplemented!("{}", cast.op),
                }
            }

            InstValue::GEP(gep) => {
                let addr = self.convert_value(gep.oprands[0].value, asm_func_id, asm_bb_id);
                let indices = gep.oprands[1..].to_vec();

                self.calc_gep(asm_func_id, asm_bb_id, addr, inst_id);
            }

            InstValue::Load(load) => {
                let addr = self.convert_value(load.oprands[0].value, asm_func_id, asm_bb_id);
                let to = self.convert_value(load, asm_func_id, asm_bb_id);

                let asm = if to.is_float() {
                    VLDRInst::new(to, addr)
                } else {
                    LoadInst::new(to, addr)
                };

                abb.insts.append(&mut self.expand_inst_imm(asm));
            }

            InstValue::Store(store) => {
                let val = self.convert_value(store.oprands[0].value, asm_func_id, asm_bb_id);
                let addr = self.convert_value(store.oprands[1].value, asm_func_id, asm_bb_id);

                let sto = if val.is_float() {
                    VSTRInst::new(val, addr)
                } else {
                    StoreInst::new(val, addr)
                };

                abb.insts.append(&mut self.expand_inst_imm(sto));
            }

            _ => unimplemented!("Unknown non-terminator instruction"),
        };
    }
    fn process_call_arg(
        &mut self,
        call_inst: &mut mc_inst::CallInst,
        mut op: AsmOperand,
        loc: AsmOperand,
        asm_bb_id: AsmValueId,
        is_lift_double: bool,
    ) {
        // If it's a constant, convert it. Ensure that all parameters are inside the register.
        let abb = self.module.get_bb_mut(asm_bb_id);
        if let AsmOperand::Imm(imm) = &op {
            let tmp = self.get_vreg(imm.is_float());
            abb.insts
                .extend(self.module.load_imm(AsmOperand::VirtReg(tmp), &imm));
            op = AsmOperand::VirtReg(tmp);
        }
        assert!(matches!(op, AsmOperand::VirtReg(_)));
        if let AsmOperand::VirtReg(vreg) = op {
            // Handle the case where vararg is lifted to double
            if is_lift_double {
                // vararg only uses BaseCallingConvention
                assert!(!matches!(loc, AsmOperand::VfpReg(_)));
                // vcvt.f64.f32 d16 // Borrow d16 which is not in the allocation range.
                let mut vcvt =
                    VCVTInst::new(VCVTType::F2D, VfpDoubleReg::default().into(), vreg.into());
                let vcvt_id = self.module.alloc_value(AsmValue::Inst(AsmInst::VCVT(vcvt)));
                abb.insts.push(vcvt_id);
                match loc {
                    AsmOperand::IntReg(reg) => {
                        // vcvt.f64.f32 d16, Sn +  vmov r2, r3, d16
                        assert!(reg.ty == RegType::R0 || reg.ty == RegType::R2);
                        let mut vmov = VMovInst::new(
                            VMovType::S2A,
                            reg.into(),
                            VfpDoubleReg::default().into(),
                        );
                        let next = IntReg {
                            ty: RegType::from(reg.ty.into() + 1),
                            is_float: false,
                        };
                        vmov.oprs.get_defs_mut().push(next.into());
                        let vmov_id = self.module.alloc_value(AsmValue::Inst(AsmInst::VMov(vmov)));
                        abb.insts.push(vmov_id);
                    }
                    AsmOperand::StackOperand(_) => {
                        // vcvt.f64.f32 d16, Sn +  vstr.64 d16, [sp]
                        let vstr = VSTRInst::new(VfpDoubleReg::default().into(), loc);
                        let vstr_id = self.module.alloc_value(AsmValue::Inst(AsmInst::VSTR(vstr)));
                        abb.insts
                            .extend(self.expand_stack_operand_load_store(vstr_id));
                    }
                    _ => panic!("Unsupported operation"),
                }
                return;
            }
            // Back to the normal case.
            match loc {
                AsmOperand::IntReg(reg) => {
                    // Maintain register allocation constraints
                    call_inst.set_in_constraint(vreg, reg.into());
                    call_inst.get_uses_mut().push(vreg);
                }
                AsmOperand::VfpReg(vfp_reg) => {
                    // Maintain register allocation constraints
                    call_inst.set_in_constraint(vreg, vfp_reg);
                    // Maintain use
                    call_inst.get_uses_mut().push(vreg);
                }
                AsmOperand::StackOperand(stack_oper) => {
                    // Generate store for parameters in memory
                    let store;
                    if vreg.is_float {
                        store = VSTRInst::new(vreg, stack_oper);
                    } else {
                        store = StoreInst::new(vreg, stack_oper);
                    }

                    abb.insts
                        .extend(self.expand_stack_operand_load_store(store));
                }
                _ => (),
            }
        }
    }

    fn calc_gep(
        &mut self, // func: &mut AsmFunc,
        asm_func_id: AsmValueId,
        asm_bb_id: AsmValueId,
        addr: AsmOperand,
        inst_id: ValueId,
    ) {
        let gep_inst = self.ir_module.get_inst(inst_id).as_gep();
        let mut base_size = gep_inst.base.get_size();
        let mut dims;

        if let Some(d) = &gep_inst.base.as_array().unwrap().dims {
            dims = d.clone();
        } else {
            dims = Vec::new();
        }

        let mut current = addr;
        let mut offset: i64 = 0;
        for get_idx in gep_inst.indices {
            let val = self.ir_module.get_value(get_idx);
            if let Value::ConstantValue(cv) = &val {
                let num = cv.val as i64;
                assert!(base_size != i64::MIN);
                offset += base_size * num;
                if !dims.is_empty() {
                    base_size = base_size / dims.remove(0);
                } else {
                    // Set as invalid
                    base_size = i64::MIN;
                }
            } else {
                if offset != 0 {
                    current = self.gep_make_add(
                        current,
                        IntImm { value: offset }.into(),
                        asm_func_id,
                        asm_bb_id,
                    );
                    offset = 0;
                }

                let mut target = self.get_vreg(false);
                let mut mul = BinOpInst::new(
                    BinaryOp::MUL,
                    target,
                    self.convert_value(&get_idx.value, asm_func_id, asm_bb_id),
                    AsmOperand::Imm(IntImm::from(base_size)),
                );
                let mut abb = self.get_abb_mut(asm_func_id, asm_bb_id);
                abb.insts.extend(self.expand_bin_op(mul));
                if !dims.is_empty() {
                    base_size = base_size / dims.remove(0);
                } else {
                    // Set as invalid
                    base_size = i64::MIN;
                }

                current = self.gep_make_add(current, target, asm_func_id, asm_bb_id);
            }
        }
        if offset != 0 {
            current = self.gep_make_add(current, IntImm::from(offset), asm_func_id, asm_bb_id);
            offset = 0;
        }

        self.vreg_map.insert(inst_id, current);
    }

    fn gep_make_add(
        &mut self,
        prev: AsmOperand,
        offset: AsmOperand,
        asm_func_id: AsmValueId,
        asm_bb_id: AsmValueId,
    ) -> AsmOperand {
        let mut target = self.get_vreg(false);
        let mut bin = BinOpInst::new(BinaryOp::ADD, target, prev, offset);
        let mut abb = self.module.get_mut_func(asm_func_id).get_mut_bb(asm_bb_id);
        abb.insts.extend(self.expand_bin_op(bin));
        target
    }
    fn get_cc(&mut self, func_id: &ValueId) -> CallConv {
        let f = self.ir_module.get_func(*func_id);
        if self.vfp_callconv_map.contains_key(func_id) {
            return CallConv::VfpCallConv(self.vfp_callconv_map.get(func_id).unwrap().clone());
        }

        let ret: VfpCallConv;
        let params: Vec<ParamInfo> = f
            .params
            .clone()
            .iter()
            .map(|v_id| {
                let param_val = FunctionValue::resolve_param(*v_id, self.ir_module);
                ParamInfo {
                    base_type: AsmTypeTag::from(param_val.ty.base_type().clone()),
                    is_pointer: param_val.ty.is_pointer(false),
                }
            })
            .collect();
        if f.is_variadic {
            // 变参函数需要在调用处临时计算CallCVfpCallConv
            // ret = BaseCallCVfpCallConv::new().resolve(&params, f.ret_type);
            unimplemented!();
        } else {
            ret = VfpCallConv::new().resolve(&params, AsmTypeTag::from(f.ret_ty.clone()));
        }

        self.vfp_callconv_map.insert(*func_id, ret.clone());
        CallConv::VfpCallConv(ret)
    }

    // 仅当处理phi指令的时候的部分情况需要 before_jump = true
    fn convert_value(
        &mut self,
        valud_id: ValueId,
        asm_func_id: AsmValueId,
        asm_bb_id: AsmValueId,
    ) -> AsmOperand {
        self.convert_value_with_before_jump(valud_id, asm_func_id, asm_bb_id, false)
    }

    // 对指令返回的值分配Vreg
    // 对其他Value进行转换
    // 该函数还需要为内存中的参数生成必要的Load指令
    fn convert_value_with_before_jump(
        &mut self,
        valud_id: ValueId,
        asm_func_id: AsmValueId,
        asm_bb_id: AsmValueId,
        before_jump: bool,
    ) -> AsmOperand {
        let asm_func_id = self.module.cur_func_value_id();
        let ssa_func_id = self.ir_module.cur_func_value_id();
        let asm_bb_id = self.module.cur_bb_value_id();
        let ssa_bb_id = self.ir_module.cur_bb_value_id();
        let ssa_func = self.ir_module.get_func(ssa_func_id);

        if self.vreg_map.contains_key(&valud_id) {
            return AsmOperand::VirtReg(self.vreg_map.get(&valud_id).unwrap().clone());
        }

        // BasicBlovkValue，FuncValue，在对应的指令预先判断处理。
        let v = self.ir_module.get_value(valud_id);
        if let Value::BasicBlock(_) | Value::Function(_) = v {
            unimplemented!();
        }

        // 如果是ConstantValue则需要转为Imm
        if let Value::Const(cv) = v {
            assert!(!cv.is_array());
            return AsmOperand::Imm(Imm::from(cv.clone()));
        }

        // IR那边GlobalVariable直接引用也代表地址，所以不用Load
        if let Value::GlobalVariable(gv) = v {
            let asmgv_id = self.gv_map.get(&valud_id).unwrap();
            let asmgv = self.module.get_global_variable(*asmgv_id);
            return AsmOperand::Imm(Imm::Label(asmgv.imm.clone()));
        }

        let ret = VirtReg {
            index: self.vreg_idx,
            is_float: *v.ty().base_type() == crate::ast::BuiltinType::Float.into(),
        };
        self.vreg_map.insert(valud_id, ret.clone().into());

        // 如果是参数且在内存中，则生成load指令
        // 使用CallCVfpCallConv的解析结果。
        if let Value::VariableValue(pv) = v {
            let arg_idx = ssa_func
                .params
                .iter()
                .position(|arg_id| *arg_id == valud_id)
                .unwrap();
            let cc = self.get_cc(&ssa_func_id); // 有函数体的必然不是vararg的。
            let loc = cc.as_vfp_call_conv().self_args[arg_idx].clone();
            assert!(loc.is_stack_operand()); // 其他的应该在前面就取到了vreg。
            if let AsmOperand::StackOperand(_) = loc {
                // 生成Load指令加载内存里的值到虚拟寄存器里。
                let load = if *pv.ty.base_type() == Type::Builtin(crate::ast::BuiltinType::Float) {
                    self.module
                        .alloc_value(AsmValue::Inst(mc_inst::AsmInst::VLDR(
                            mc_inst::VLDRInst::new(ret.clone().into(), loc),
                        )))
                } else {
                    self.module
                        .alloc_value(AsmValue::Inst(mc_inst::AsmInst::LDR(
                            mc_inst::LDRInst::new(ret.clone().into(), loc),
                        )))
                };
                if before_jump {
                    let new_insts = self.expand_stack_operand_load_store(load);
                    self.module.add_all_before_branch(asm_bb_id, new_insts);
                } else {
                    let new_insts = self.expand_stack_operand_load_store(load);
                    let abb = self.module.get_bb_mut(asm_bb_id);
                    abb.insts.extend(new_insts);
                }
            }
        }

        // 增加注释便于Debug

        AsmOperand::VirtReg(ret)
    }
    // Check whether the second argument StackOperand meets the requirements, if not, expand it into multiple instructions
    // Load dst, addr
    // Store val, addr
    fn expand_stack_operand_load_store(&mut self, inst_id: AsmValueId) -> Vec<AsmValueId> {
        let mut ret = Vec::new();
        let mut newuse = Vec::new();

        let mut inst = self.module.get_inst(inst_id).clone();
        assert!(inst.is_ldr() || inst.is_str() || inst.is_vstr() || inst.is_vldr());

        if inst.is_str() || inst.is_vstr() {
            newuse.push(inst.get_uses()[0].clone());
            self.expand_stack_operand(inst_id, &inst.get_uses()[1], &mut newuse, &mut ret);
        } else if inst.is_ldr() || inst.is_vldr() {
            self.expand_stack_operand(inst_id, &inst.get_uses()[0], &mut newuse, &mut ret);
        }
        inst.set_uses(newuse);
        self.module.set_inst(inst_id, inst);
        ret.push(inst_id);
        ret
    }

    fn expand_cmp_imm(&mut self, inst_id: AsmValueId) -> Vec<AsmValueId> {
        let mut inst = self.module.get_inst_mut(inst_id).clone();
        let mut ret = Vec::new();
        let mut newuse = Vec::new();
        self.expand_imm(&inst.get_uses()[0], &mut newuse, &mut ret);
        self.expand_operand2(&inst.get_uses()[1], &mut newuse, &mut ret);
        inst.set_uses(newuse);
        self.module.set_inst(inst_id, inst);
        ret.push(inst_id);
        ret
    }

    fn expand_inst_imm(&mut self, inst_id: AsmValueId) -> Vec<AsmValueId> {
        let mut inst = self.module.get_inst_mut(inst_id).clone();
        let mut ret = Vec::new();
        let mut newuse = Vec::new();
        for op in &inst.get_uses() {
            self.expand_imm(op, &mut newuse, &mut ret);
        }
        inst.set_uses(newuse);
        self.module.set_inst(inst_id, inst);
        ret.push(inst_id);
        ret
    }

    fn expand_imm(
        &mut self,
        op: &AsmOperand,
        new_ops: &mut Vec<AsmOperand>,
        insts: &mut Vec<AsmValueId>,
    ) {
        if let AsmOperand::Imm(imm) = op {
            let tmp = self.get_vreg(op.is_float());
            insts.extend(self.module.load_imm(tmp.clone().into(), &imm));
            new_ops.push(tmp.into());
        } else {
            new_ops.push((*op).clone());
        }
    }

    fn expand_stack_operand(
        &mut self,
        inst_id: AsmValueId,
        op: &AsmOperand,
        new_ops: &mut Vec<AsmOperand>,
        insts: &mut Vec<AsmValueId>,
    ) {
        if let AsmOperand::StackOperand(so) = op {
            let inst = self.module.get_inst(inst_id);
            if inst.is_imm_fit(so) {
                new_ops.push(AsmOperand::StackOperand(so.clone()));
                return;
            }
            let tmp = AsmOperand::VirtReg(self.get_vreg(false));
            let tmp2 = AsmOperand::VirtReg(self.get_vreg(false));
            assert!(so.ty != StackOperandType::Spill);
            let id = match so.ty {
                StackOperandType::SelfArg => {
                    insts.extend(
                        self.module
                            .load_imm(tmp.clone(), &Imm::Int(IntImm::from(so.offset as i32))),
                    );
                    let binop = mc_inst::BinOpInst::new(
                        mc_inst::BinaryOp::Add,
                        tmp2.clone(),
                        IntReg::new(RegType::Fp).into(),
                        tmp,
                    );
                    let new_inst = AsmValue::Inst(mc_inst::AsmInst::BinOp(binop));
                    self.module.alloc_value(new_inst)
                }
                StackOperandType::Local => {
                    insts.extend(
                        self.module
                            .load_imm(tmp.clone(), &Imm::Int(IntImm::from(so.offset as i32))),
                    );
                    let binop = mc_inst::BinOpInst::new(
                        mc_inst::BinaryOp::Sub,
                        tmp2.clone(),
                        IntReg::new(RegType::Fp).into(),
                        tmp,
                    );
                    let new_inst = AsmValue::Inst(mc_inst::AsmInst::BinOp(binop));
                    self.module.alloc_value(new_inst)
                }
                StackOperandType::CallParam => {
                    insts.extend(
                        self.module
                            .load_imm(tmp.clone(), &Imm::Int(IntImm::from(so.offset as i32))),
                    );
                    let binop = mc_inst::BinOpInst::new(
                        mc_inst::BinaryOp::Add,
                        tmp2.clone(),
                        IntReg::new(RegType::Sp).into(),
                        tmp,
                    );
                    let new_inst = AsmValue::Inst(mc_inst::AsmInst::BinOp(binop));
                    self.module.alloc_value(new_inst)
                }
                _ => panic!("Unsupported operation"),
            };
            insts.push(id);
            new_ops.push(tmp2);
        } else {
            new_ops.push((*op).clone());
        }
    }
    // Flexible Operand 2 can be Imm8m
    fn expand_operand2(
        &mut self,
        op: &AsmOperand,
        new_ops: &mut Vec<AsmOperand>,
        insts: &mut Vec<AsmValueId>,
    ) {
        match op {
            AsmOperand::Imm(immop) => {
                if immop.highest_one_bit() < 255 {
                    new_ops.push(AsmOperand::Imm(immop.clone()));
                    return;
                }
                let tmp = AsmOperand::VirtReg(self.get_vreg(false));
                insts.extend(self.module.load_imm(tmp.clone(), immop));
                new_ops.push(tmp);
            }
            _ => {
                new_ops.push((*op).clone());
            }
        }
    }

    pub fn expand_bin_op(&mut self, bin_id: AsmValueId) -> Vec<AsmValueId> {
        let mut bin_inst = self
            .module
            .get_inst_mut(bin_id)
            .as_bin_op()
            .unwrap()
            .clone();
        let mut ret = Vec::new();
        let mut op1 = bin_inst.get_uses()[0].clone();
        let mut op2 = bin_inst.get_uses()[1].clone();
        if let AsmOperand::Imm(imm) = op1.clone() {
            let tmp = AsmOperand::VirtReg(self.get_vreg(op1.is_float()));
            ret.extend(self.module.load_imm(tmp.clone(), &imm));
            op1 = tmp;
        }
        match op2.clone() {
            AsmOperand::Imm(imm) => {
                if !matches!(bin_inst.op, mc_inst::BinaryOp::Add | mc_inst::BinaryOp::Sub)
                    || imm.highest_one_bit() >= 255
                {
                    let tmp = AsmOperand::VirtReg(self.get_vreg(op2.is_float()));
                    ret.extend(self.module.load_imm(tmp.clone(), &imm));
                    op2 = tmp;
                }
            }
            _ => (),
        }
        bin_inst.set_uses(vec![op1, op2]);
        self.module.set_inst(bin_id, bin_inst.into());
        ret.push(bin_id);
        ret
    }

    // 检查第二个参数StackOperand是否满足要求，不满足则展开为多个指令
    // 给寄存器分配使用的公开版本
    pub fn expand_stack_operand_load_store_ip(&mut self, inst_id: AsmValueId) -> Vec<AsmValueId> {
        let inst = self.module.get_inst_mut(inst_id).clone();
        let mut ret = Vec::<AsmValueId>::new();
        let mut newuse = Vec::new();
        assert!(matches!(
            inst,
            AsmInst::LDR(_) | AsmInst::STR(_) | AsmInst::VSTR(_) | AsmInst::VLDR(_)
        ));
        match inst {
            AsmInst::STR(_) | AsmInst::VSTR(_) => {
                newuse.push(inst.get_uses()[0].clone());
                self.expand_stack_operand_ip(
                    inst_id,
                    &inst.get_uses()[1],
                    &inst.get_uses()[0],
                    &mut newuse,
                    &mut ret,
                );
            }
            AsmInst::LDR(_) | AsmInst::VLDR(_) => {
                self.expand_stack_operand_ip(
                    inst_id,
                    &inst.get_uses()[0],
                    &inst.get_defs()[0],
                    &mut newuse,
                    &mut ret,
                );
            }
            _ => (),
        }
        // in case of using a old value
        let mut inst = self.module.get_inst_mut(inst_id).clone();
        inst.set_uses(newuse);
        self.module.set_inst(inst_id, inst);
        ret.push(inst_id);
        ret
    }

    pub fn expand_stack_operand_ip(
        &mut self,
        inst_id: AsmValueId,
        op: &AsmOperand,
        target: &AsmOperand,
        new_ops: &mut Vec<AsmOperand>,
        insts: &mut Vec<AsmValueId>,
    ) {
        let inst = self.module.get_inst(inst_id).clone();
        match op {
            AsmOperand::StackOperand(so) => {
                if inst.is_imm_fit(so) {
                    new_ops.push(AsmOperand::StackOperand(so.clone()));
                    return;
                }
                assert!(matches!(
                    target,
                    AsmOperand::IntReg(_) | AsmOperand::VfpReg(_)
                ));
                let tmp = AsmOperand::IntReg(IntReg::new(RegType::Ip));
                let tmp2 = AsmOperand::IntReg(IntReg::new(RegType::Ip));
                match so.ty {
                    StackOperandType::SelfArg => {
                        insts.extend(
                            self.module
                                .load_imm(tmp.clone(), &Imm::Int(IntImm::from(so.offset as i32))),
                        );
                        let inst = mc_inst::BinOpInst::new(
                            mc_inst::BinaryOp::Add,
                            tmp2.clone(),
                            AsmOperand::IntReg(IntReg::new(RegType::Fp)),
                            tmp.clone(),
                        );
                        let inst = AsmValue::Inst(AsmInst::BinOp(inst));
                        let id = self.module.alloc_value(inst);

                        insts.push(id);
                    }
                    StackOperandType::Local | StackOperandType::Spill => {
                        insts.extend(
                            self.module
                                .load_imm(tmp.clone(), &Imm::Int(IntImm::from(so.offset as i32))),
                        );
                        let inst = mc_inst::BinOpInst::new(
                            mc_inst::BinaryOp::Sub,
                            tmp2.clone(),
                            AsmOperand::IntReg(IntReg::new(RegType::Fp)),
                            tmp.clone(),
                        );
                        let inst = AsmValue::Inst(AsmInst::BinOp(inst));
                        let id = self.module.alloc_value(inst);

                        insts.push(id);
                    }
                    StackOperandType::CallParam => {
                        insts.extend(
                            self.module
                                .load_imm(tmp.clone(), &Imm::Int(IntImm::from(so.offset as i32))),
                        );
                        let inst = mc_inst::BinOpInst::new(
                            mc_inst::BinaryOp::Add,
                            tmp2.clone(),
                            AsmOperand::IntReg(IntReg::new(RegType::Sp)),
                            tmp.clone(),
                        );
                        let inst = AsmValue::Inst(AsmInst::BinOp(inst));
                        let id = self.module.alloc_value(inst);

                        insts.push(id);
                    }
                    _ => panic!("Unsupported operation"),
                }
                new_ops.push(tmp2);
            }
            _ => {
                new_ops.push((*op).clone());
            }
        }
    }
    // 使用临时寄存器的场景
    fn get_vreg(&mut self, is_float: bool) -> VirtReg {
        let ret = VirtReg::new(self.vreg_idx, is_float);
        self.vreg_idx += 1;
        ret
    }
    // fn build_basic_blocks(&mut self, blocks: &Vec<BasicBlock>) -> Vec<asmBasicBlock> {
    //     let mut asm_blocks = Vec::new();
    //     for block in blocks {
    //         asm_blocks.push(self.build_basic_block(block));
    //     }
    //     asm_blocks
    // }

    // fn build_basic_block(&mut self, block: &BasicBlock) -> asmBasicBlock {
    //     let mut asm_block = asmBasicBlock::new();
    //     asm_block.name = block.name.clone();
    //     asm_block.instructions = self.build_instructions(&block.instructions);
    //     asm_block
    // }

    // fn build_instructions(&mut self, instructions: &Vec<Instruction>) -> Vec<asmInstruction> {
    //     let mut asm_instructions = Vec::new();
    //     for instruction in instructions {
    //         asm_instructions.push(self.build_instruction(instruction));
    //     }
    //     asm_instructions
    // }

    // fn build_instruction(&mut self, instruction: &Instruction) -> asmInstruction {
    //     let mut asm_instruction = asmInstruction::new();
    //     asm_instruction.opcode = instruction.opcode.clone();
    //     asm_instruction.operands = self.build_operands(&instruction.operands);
    //     asm_instruction
    // }

    // fn build_operands(&mut self, operands: &Vec<Operand>) -> Vec<asmOperand> {
    //     let mut asm_operands = Vec::new();
    //     for operand in operands {
    //         asm_operands.push(self.build_operand(operand));
    //     }
    //     asm_operands
    // }

    // fn build_operand(&mut self, operand: &Operand) -> asmOperand {
    //     todo!()
    // }
}
