use std::collections::HashMap;
use std::path::Path;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::{InitializationConfig, Target, TargetMachine};
use inkwell::types::{BasicTypeEnum, StructType};
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};
use inkwell::{AddressSpace, OptimizationLevel};

use crate::ast::*;

#[derive(Debug, Clone)]
struct FnSig {
    params: Vec<Type>,
    ret: Type,
}

#[derive(Debug, Clone)]
struct StructInfo {
    fields: Vec<(String, Type)>,
    methods: HashMap<String, FnSig>,
}

#[derive(Clone)]
struct CgValue<'ctx> {
    value: Option<BasicValueEnum<'ctx>>,
    ty: Type,
}

pub struct Codegen<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    structs: HashMap<String, StructType<'ctx>>,
    functions: HashMap<String, FunctionValue<'ctx>>,
    var_scopes: Vec<HashMap<String, PointerValue<'ctx>>>,
    type_scopes: Vec<HashMap<String, Type>>,
    sigs: HashMap<String, FnSig>,
    struct_info: HashMap<String, StructInfo>,
    current_fn: Option<FunctionValue<'ctx>>,
    current_ret: Option<Type>,
}

impl<'ctx> Codegen<'ctx> {
    pub fn new(context: &'ctx Context, module_name: &str) -> Self {
        let module = context.create_module(module_name);
        let builder = context.create_builder();
        Self {
            context,
            module,
            builder,
            structs: HashMap::new(),
            functions: HashMap::new(),
            var_scopes: vec![],
            type_scopes: vec![],
            sigs: HashMap::new(),
            struct_info: HashMap::new(),
            current_fn: None,
            current_ret: None,
        }
    }

    pub fn module(&self) -> &Module<'ctx> {
        &self.module
    }

    pub fn write_ir(module: &Module<'ctx>, path: &Path) -> Result<(), String> {
        module
            .print_to_file(path)
            .map_err(|e| format!("write ir failed: {e}"))
    }

    pub fn write_object(module: &Module<'ctx>, path: &Path) -> Result<(), String> {
        Target::initialize_all(&InitializationConfig::default());
        let target = Target::from_triple(&TargetMachine::get_default_triple())
            .map_err(|e| format!("target init failed: {e}"))?;
        let machine = target
            .create_target_machine(
                &TargetMachine::get_default_triple(),
                "generic",
                "",
                OptimizationLevel::None,
                inkwell::targets::RelocMode::Default,
                inkwell::targets::CodeModel::Default,
            )
            .ok_or("create target machine failed")?;
        machine
            .write_to_file(module, inkwell::targets::FileType::Object, path)
            .map_err(|e| format!("write object failed: {e}"))
    }

    pub fn collect_signatures(&mut self, items: &[Item]) -> Result<(), String> {
        self.init_builtins();
        for item in items {
            match item {
                Item::Fn(f) => {
                    if self.sigs.contains_key(&f.name) {
                        return Err(format!("function '{}' already defined", f.name));
                    }
                    self.sigs.insert(
                        f.name.clone(),
                        FnSig {
                            params: f.params.iter().map(|p| p.ty.clone()).collect(),
                            ret: f.ret.clone(),
                        },
                    );
                }
                Item::Struct(_) => {}
                Item::Impl(imp) => {
                    let info = self
                        .struct_info
                        .get_mut(&imp.type_name)
                        .ok_or_else(|| format!("impl for unknown struct '{}'", imp.type_name))?;
                    for method in &imp.methods {
                        let sig = FnSig {
                            params: method.params.iter().map(|p| p.ty.clone()).collect(),
                            ret: method.ret.clone(),
                        };
                        info.methods.insert(method.name.clone(), sig.clone());
                        self.sigs.insert(
                            format!("{}__{}", imp.type_name, method.name),
                            sig,
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn collect_structs(&mut self, items: &[Item]) -> Result<(), String> {
        for item in items {
            if let Item::Struct(def) = item {
                if self.structs.contains_key(&def.name) {
                    return Err(format!("duplicate struct '{}'", def.name));
                }
                let st = self.context.opaque_struct_type(&def.name);
                self.structs.insert(def.name.clone(), st);
                self.struct_info.insert(
                    def.name.clone(),
                    StructInfo {
                        fields: def
                            .fields
                            .iter()
                            .map(|f| (f.name.clone(), f.ty.clone()))
                            .collect(),
                        methods: HashMap::new(),
                    },
                );
            }
        }
        for item in items {
            if let Item::Struct(def) = item {
                let st = self
                    .structs
                    .get(&def.name)
                    .copied()
                    .ok_or_else(|| format!("missing struct '{}'", def.name))?;
                let field_types: Vec<BasicTypeEnum<'ctx>> = def
                    .fields
                    .iter()
                    .map(|f| self.llvm_type(&f.ty))
                    .collect();
                st.set_body(&field_types, false);
            }
        }
        Ok(())
    }

    fn declare_functions(&mut self, items: &[Item]) -> Result<(), String> {
        for item in items {
            match item {
                Item::Fn(f) => {
                    self.declare_function(&f.name, &f.params, &f.ret, None)?;
                }
                Item::Impl(imp) => {
                    for method in &imp.methods {
                        let name = format!("{}__{}", imp.type_name, method.name);
                        self.declare_function(&name, &method.params, &method.ret, Some(&imp.type_name))?;
                    }
                }
                Item::Struct(_) => {}
            }
        }
        Ok(())
    }

    fn declare_function(
        &mut self,
        name: &str,
        params: &[Param],
        ret: &Type,
        self_type: Option<&str>,
    ) -> Result<(), String> {
        let mut param_types: Vec<BasicTypeEnum<'ctx>> = vec![];
        for p in params {
            let ty = self.llvm_type_with_self(&p.ty, self_type);
            param_types.push(ty);
        }
        let fn_type = match ret {
            Type::Void => self.context.void_type().fn_type(&param_types, false),
            _ => self.llvm_type_with_self(ret, self_type).fn_type(&param_types, false),
        };
        let f = self.module.add_function(name, fn_type, None);
        self.functions.insert(name.to_string(), f);
        Ok(())
    }

    fn init_builtins(&mut self) {
        self.sigs.insert(
            "print".into(),
            FnSig {
                params: vec![Type::Str],
                ret: Type::Void,
            },
        );
        self.sigs.insert(
            "input".into(),
            FnSig {
                params: vec![],
                ret: Type::Str,
            },
        );
        self.sigs.insert(
            "len".into(),
            FnSig {
                params: vec![Type::Str],
                ret: Type::I32,
            },
        );
        self.sigs.insert(
            "str".into(),
            FnSig {
                params: vec![Type::I32],
                ret: Type::Str,
            },
        );
    }

    fn declare_runtime(&mut self) -> Result<(), String> {
        let i8ptr = self.context.i8_type().ptr_type(AddressSpace::default());
        let i32t = self.context.i32_type();
        let voidt = self.context.void_type();

        let print = voidt.fn_type(&[i8ptr.into()], false);
        self.module.add_function("pebbles_print_str", print, None);

        let input = i8ptr.fn_type(&[], false);
        self.module.add_function("pebbles_input", input, None);

        let len = i32t.fn_type(&[i8ptr.into()], false);
        self.module.add_function("pebbles_len_str", len, None);

        let str_i32 = i8ptr.fn_type(&[i32t.into()], false);
        self.module.add_function("pebbles_str_i32", str_i32, None);

        let concat = i8ptr.fn_type(&[i8ptr.into(), i8ptr.into()], false);
        self.module
            .add_function("pebbles_str_concat", concat, None);
        Ok(())
    }

    fn define_items(&mut self, items: &[Item]) -> Result<(), String> {
        for item in items {
            match item {
                Item::Fn(f) => self.codegen_fn(f, None)?,
                Item::Impl(imp) => {
                    for method in &imp.methods {
                        self.codegen_fn(method, Some(&imp.type_name))?;
                    }
                }
                Item::Struct(_) => {}
            }
        }
        Ok(())
    }

    fn codegen_fn(&mut self, f: &FnDef, self_type: Option<&str>) -> Result<(), String> {
        let name = match self_type {
            Some(ty) => format!("{}__{}", ty, f.name),
            None => f.name.clone(),
        };
        let func = *self
            .functions
            .get(&name)
            .ok_or_else(|| format!("missing function '{name}'"))?;

        let entry = self.context.append_basic_block(func, "entry");
        self.builder.position_at_end(entry);
        self.current_fn = Some(func);
        self.current_ret = Some(f.ret.clone());

        self.push_scope();

        for (idx, param) in f.params.iter().enumerate() {
            let llvm_param = func
                .get_nth_param(idx as u32)
                .ok_or_else(|| format!("missing param {idx} in '{name}'"))?;
            let param_ty = if param.ty == Type::SelfType {
                Type::Named(self_type.unwrap_or("self").to_string())
            } else {
                param.ty.clone()
            };
            let alloca = self.create_entry_alloca(&param.name, &param_ty)?;
            self.builder.build_store(alloca, llvm_param);
            self.bind_var(&param.name, param_ty, alloca);
        }

        for stmt in &f.body {
            self.codegen_stmt(stmt)?;
        }

        if let Some(block) = self.builder.get_insert_block() {
            if block.get_terminator().is_none() {
                match &f.ret {
                    Type::Void => {
                        self.builder.build_return(None);
                    }
                    _ => return Err(format!("missing return in function '{}'", name)),
                }
            }
        }

        self.pop_scope();
        self.current_fn = None;
        self.current_ret = None;
        Ok(())
    }

    fn codegen_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::Let {
                name,
                ty,
                value,
                ..
            } => {
                let expected = ty.as_ref();
                let rhs = self.codegen_expr(value, expected)?;
                let var_ty = ty.clone().unwrap_or(rhs.ty.clone());
                let alloca = self.create_entry_alloca(name, &var_ty)?;
                if let Some(val) = rhs.value {
                    self.builder.build_store(alloca, val);
                }
                self.bind_var(name, var_ty, alloca);
                Ok(())
            }
            Stmt::Assign { target, value, .. } => {
                let (ptr, target_ty) = self.codegen_assign_target(target)?;
                let rhs = self.codegen_expr(value, Some(&target_ty))?;
                let val = rhs
                    .value
                    .ok_or_else(|| "assignment expects value".to_string())?;
                self.builder.build_store(ptr, val);
                Ok(())
            }
            Stmt::Return { value, .. } => {
                let ret_ty = self.current_ret.clone().unwrap_or(Type::Void);
                let ret_val = if let Some(expr) = value {
                    let v = self.codegen_expr(expr, Some(&ret_ty))?;
                    v.value
                        .ok_or_else(|| "return expects value".to_string())?
                } else {
                    self.builder.build_return(None);
                    return Ok(());
                };
                self.builder.build_return(Some(&ret_val));
                Ok(())
            }
            Stmt::Expr(expr) => {
                let _ = self.codegen_expr(expr, None)?;
                Ok(())
            }
            Stmt::If { .. }
            | Stmt::While { .. }
            | Stmt::For { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. } => Err("control flow not codegenned yet".into()),
        }
    }

    fn codegen_expr(&mut self, expr: &Expr, expected: Option<&Type>) -> Result<CgValue<'ctx>, String> {
        match expr {
            Expr::Int(n, _) => Ok(CgValue {
                value: Some(self.context.i32_type().const_int(*n as u64, true).into()),
                ty: Type::I32,
            }),
            Expr::Float(f, _) => Ok(CgValue {
                value: Some(self.context.f64_type().const_float(*f).into()),
                ty: Type::F64,
            }),
            Expr::Bool(b, _) => Ok(CgValue {
                value: Some(self.context.bool_type().const_int(u64::from(*b), false).into()),
                ty: Type::Bool,
            }),
            Expr::Str(s, _) => {
                let gv = self
                    .builder
                    .build_global_string_ptr(s, "str")
                    .as_pointer_value()
                    .into();
                Ok(CgValue {
                    value: Some(gv),
                    ty: Type::Str,
                })
            }
            Expr::None(_) => {
                let opt_ty = expected
                    .cloned()
                    .ok_or_else(|| "none requires optional type".to_string())?;
                let llvm_ty = self.llvm_type(&opt_ty);
                let val = llvm_ty.into_struct_type().get_undef();
                Ok(CgValue {
                    value: Some(val.into()),
                    ty: opt_ty,
                })
            }
            Expr::Ident(name, _) => {
                let ptr = self.lookup_var(name)?;
                let ty = self.lookup_var_type(name)?;
                let val = self.builder.build_load(ptr, name).into();
                Ok(CgValue {
                    value: Some(val),
                    ty,
                })
            }
            Expr::Tuple(elems, _) => {
                let mut vals = Vec::new();
                let mut tys = Vec::new();
                for e in elems {
                    let cv = self.codegen_expr(e, None)?;
                    let v = cv.value.ok_or_else(|| "tuple expects value".to_string())?;
                    vals.push(v);
                    tys.push(cv.ty);
                }
                let tuple_ty = Type::Tuple(tys.clone());
                let llvm_ty = self.llvm_type(&tuple_ty).into_struct_type();
                let mut cur = llvm_ty.get_undef();
                for (idx, v) in vals.iter().enumerate() {
                    cur = self.builder.build_insert_value(cur, *v, idx as u32, "tup").unwrap().into_struct_value();
                }
                Ok(CgValue {
                    value: Some(cur.into()),
                    ty: tuple_ty,
                })
            }
            Expr::Range { start, end, inclusive, .. } => {
                let s = self.codegen_expr(start, Some(&Type::I32))?;
                let e = self.codegen_expr(end, Some(&Type::I32))?;
                let s_val = s.value.ok_or_else(|| "range start expects value".to_string())?;
                let e_val = e.value.ok_or_else(|| "range end expects value".to_string())?;
                let i1 = self.context.bool_type().const_int(u64::from(*inclusive), false);
                let range_ty = self.llvm_type(&Type::Range).into_struct_type();
                let mut cur = range_ty.get_undef();
                cur = self.builder.build_insert_value(cur, s_val, 0, "range").unwrap().into_struct_value();
                cur = self.builder.build_insert_value(cur, e_val, 1, "range").unwrap().into_struct_value();
                cur = self.builder.build_insert_value(cur, i1, 2, "range").unwrap().into_struct_value();
                Ok(CgValue {
                    value: Some(cur.into()),
                    ty: Type::Range,
                })
            }
            Expr::StructLit { name, fields, .. } => {
                let info = self
                    .struct_info
                    .get(name)
                    .ok_or_else(|| format!("unknown struct '{name}'"))?
                    .clone();
                let st_ty = self
                    .structs
                    .get(name)
                    .copied()
                    .ok_or_else(|| format!("missing struct '{name}'"))?;
                let mut cur = st_ty.get_undef();
                for (idx, (field_name, field_ty)) in info.fields.iter().enumerate() {
                    let expr = fields
                        .iter()
                        .find(|(n, _)| n == field_name)
                        .ok_or_else(|| format!("missing field '{field_name}'"))?
                        .1
                        .clone();
                    let val = self.codegen_expr(&expr, Some(field_ty))?;
                    let v = val.value.ok_or_else(|| "struct field expects value".to_string())?;
                    cur = self
                        .builder
                        .build_insert_value(cur, v, idx as u32, "field")
                        .unwrap()
                        .into_struct_value();
                }
                Ok(CgValue {
                    value: Some(cur.into()),
                    ty: Type::Named(name.clone()),
                })
            }
            Expr::FieldAccess { obj, field, .. } => {
                let (ptr, field_ty) = self.codegen_field_ptr(obj, field)?;
                let val = self.builder.build_load(ptr, field);
                Ok(CgValue {
                    value: Some(val.into()),
                    ty: field_ty,
                })
            }
            Expr::BinOp { op, left, right, .. } => {
                let lt = self.infer_expr_type(left)?;
                let lv = self.codegen_expr(left, Some(&lt))?;
                let rv = self.codegen_expr(right, Some(&lt))?;
                let l = lv.value.ok_or_else(|| "binop expects value".to_string())?;
                let r = rv.value.ok_or_else(|| "binop expects value".to_string())?;
                let res = self.codegen_binop(*op, l, r, &lt)?;
                Ok(CgValue {
                    value: Some(res),
                    ty: self.infer_binop_type(*op, &lt),
                })
            }
            Expr::UnaryOp { op, operand, .. } => {
                let ot = self.infer_expr_type(operand)?;
                let ov = self.codegen_expr(operand, Some(&ot))?;
                let v = ov.value.ok_or_else(|| "unary expects value".to_string())?;
                let res = match op {
                    UnaryOp::Neg => {
                        if ot == Type::F64 {
                            self.builder.build_float_neg(v.into_float_value(), "fneg").into()
                        } else {
                            self.builder.build_int_neg(v.into_int_value(), "ineg").into()
                        }
                    }
                    UnaryOp::Not => self.builder.build_not(v.into_int_value(), "not").into(),
                };
                Ok(CgValue { value: Some(res), ty: ot })
            }
            Expr::Call { name, args, .. } => {
                let sig = self
                    .sigs
                    .get(name)
                    .ok_or_else(|| format!("unknown function '{name}'"))?
                    .clone();
                let mut llvm_args = Vec::new();
                for (arg, param_ty) in args.iter().zip(sig.params.iter()) {
                    let cv = self.codegen_expr(arg, Some(param_ty))?;
                    let v = cv.value.ok_or_else(|| "call expects value".to_string())?;
                    llvm_args.push(v);
                }
                let f = self
                    .functions
                    .get(name)
                    .copied()
                    .ok_or_else(|| format!("missing function '{name}'"))?;
                let call = self.builder.build_call(f, &llvm_args, "call");
                let ret = if sig.ret == Type::Void {
                    None
                } else {
                    Some(call.try_as_basic_value().left().unwrap())
                };
                Ok(CgValue { value: ret, ty: sig.ret })
            }
            Expr::MethodCall { obj, method, args, .. } => {
                let obj_ty = self.infer_expr_type(obj)?;
                let struct_name = match obj_ty {
                    Type::Named(n) => n,
                    _ => return Err("method call on non-struct".into()),
                };
                let f_name = format!("{}__{}", struct_name, method);
                let f = self
                    .functions
                    .get(&f_name)
                    .copied()
                    .ok_or_else(|| format!("missing method '{f_name}'"))?;
                let mut llvm_args = Vec::new();
                let obj_val = self.codegen_expr(obj, None)?;
                let obj_arg = obj_val.value.ok_or_else(|| "method expects value".to_string())?;
                llvm_args.push(obj_arg);
                for arg in args {
                    let cv = self.codegen_expr(arg, None)?;
                    let v = cv.value.ok_or_else(|| "method arg expects value".to_string())?;
                    llvm_args.push(v);
                }
                let call = self.builder.build_call(f, &llvm_args, "mcall");
                let sig = self
                    .sigs
                    .get(&f_name)
                    .ok_or_else(|| format!("missing method sig '{f_name}'"))?
                    .clone();
                let ret = if sig.ret == Type::Void {
                    None
                } else {
                    Some(call.try_as_basic_value().left().unwrap())
                };
                Ok(CgValue { value: ret, ty: sig.ret })
            }
            Expr::Cast { expr, ty, .. } => {
                let from_ty = self.infer_expr_type(expr)?;
                let v = self.codegen_expr(expr, Some(&from_ty))?;
                let val = v.value.ok_or_else(|| "cast expects value".to_string())?;
                let res = self.codegen_cast(val, &from_ty, ty)?;
                Ok(CgValue {
                    value: Some(res),
                    ty: ty.clone(),
                })
            }
            Expr::If { .. } | Expr::Match { .. } | Expr::Array(_, _) | Expr::Index { .. } => {
                Err("expression not codegenned yet".into())
            }
        }
    }

    fn codegen_binop(
        &self,
        op: BinOp,
        l: BasicValueEnum<'ctx>,
        r: BasicValueEnum<'ctx>,
        ty: &Type,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                if *ty == Type::F64 {
                    let lv = l.into_float_value();
                    let rv = r.into_float_value();
                    let v = match op {
                        BinOp::Add => self.builder.build_float_add(lv, rv, "fadd"),
                        BinOp::Sub => self.builder.build_float_sub(lv, rv, "fsub"),
                        BinOp::Mul => self.builder.build_float_mul(lv, rv, "fmul"),
                        BinOp::Div => self.builder.build_float_div(lv, rv, "fdiv"),
                        BinOp::Mod => self.builder.build_float_rem(lv, rv, "frem"),
                        _ => unreachable!(),
                    };
                    Ok(v.into())
                } else if *ty == Type::Str && op == BinOp::Add {
                    let f = self
                        .module
                        .get_function("pebbles_str_concat")
                        .ok_or_else(|| "missing pebbles_str_concat".to_string())?;
                    let call = self.builder.build_call(f, &[l, r], "concat");
                    Ok(call.try_as_basic_value().left().unwrap())
                } else {
                    let lv = l.into_int_value();
                    let rv = r.into_int_value();
                    let v = match op {
                        BinOp::Add => self.builder.build_int_add(lv, rv, "iadd"),
                        BinOp::Sub => self.builder.build_int_sub(lv, rv, "isub"),
                        BinOp::Mul => self.builder.build_int_mul(lv, rv, "imul"),
                        BinOp::Div => self.builder.build_int_signed_div(lv, rv, "idiv"),
                        BinOp::Mod => self.builder.build_int_signed_rem(lv, rv, "irem"),
                        _ => unreachable!(),
                    };
                    Ok(v.into())
                }
            }
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
                if *ty == Type::F64 {
                    let lv = l.into_float_value();
                    let rv = r.into_float_value();
                    let pred = match op {
                        BinOp::Eq => inkwell::FloatPredicate::OEQ,
                        BinOp::NotEq => inkwell::FloatPredicate::ONE,
                        BinOp::Lt => inkwell::FloatPredicate::OLT,
                        BinOp::Gt => inkwell::FloatPredicate::OGT,
                        BinOp::LtEq => inkwell::FloatPredicate::OLE,
                        BinOp::GtEq => inkwell::FloatPredicate::OGE,
                        _ => unreachable!(),
                    };
                    Ok(self.builder.build_float_compare(pred, lv, rv, "fcmp").into())
                } else {
                    let lv = l.into_int_value();
                    let rv = r.into_int_value();
                    let pred = match op {
                        BinOp::Eq => inkwell::IntPredicate::EQ,
                        BinOp::NotEq => inkwell::IntPredicate::NE,
                        BinOp::Lt => inkwell::IntPredicate::SLT,
                        BinOp::Gt => inkwell::IntPredicate::SGT,
                        BinOp::LtEq => inkwell::IntPredicate::SLE,
                        BinOp::GtEq => inkwell::IntPredicate::SGE,
                        _ => unreachable!(),
                    };
                    Ok(self.builder.build_int_compare(pred, lv, rv, "icmp").into())
                }
            }
            BinOp::And | BinOp::Or => {
                let lv = l.into_int_value();
                let rv = r.into_int_value();
                let v = match op {
                    BinOp::And => self.builder.build_and(lv, rv, "and"),
                    BinOp::Or => self.builder.build_or(lv, rv, "or"),
                    _ => unreachable!(),
                };
                Ok(v.into())
            }
        }
    }

    fn codegen_cast(
        &self,
        val: BasicValueEnum<'ctx>,
        from: &Type,
        to: &Type,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match (from, to) {
            (Type::I32, Type::F64) | (Type::I64, Type::F64) => Ok(self
                .builder
                .build_signed_int_to_float(val.into_int_value(), self.context.f64_type(), "sitofp")
                .into()),
            (Type::F64, Type::I32) => Ok(self
                .builder
                .build_float_to_signed_int(val.into_float_value(), self.context.i32_type(), "fptosi")
                .into()),
            (Type::F64, Type::I64) => Ok(self
                .builder
                .build_float_to_signed_int(val.into_float_value(), self.context.i64_type(), "fptosi")
                .into()),
            (Type::I32, Type::I64) => Ok(self
                .builder
                .build_int_cast(val.into_int_value(), self.context.i64_type(), "sext")
                .into()),
            (Type::I64, Type::I32) => Ok(self
                .builder
                .build_int_cast(val.into_int_value(), self.context.i32_type(), "trunc")
                .into()),
            _ => Err(format!("unsupported cast {:?} -> {:?}", from, to)),
        }
    }

    fn codegen_assign_target(
        &mut self,
        target: &AssignTarget,
    ) -> Result<(PointerValue<'ctx>, Type), String> {
        match target {
            AssignTarget::Ident(name, _) => {
                let ptr = self.lookup_var(name)?;
                let ty = self.lookup_var_type(name)?;
                Ok((ptr, ty))
            }
            AssignTarget::Field { obj, field, .. } => {
                let (ptr, ty) = self.codegen_field_ptr(obj, field)?;
                Ok((ptr, ty))
            }
            AssignTarget::Index { .. } => Err("index assignment not codegenned yet".into()),
        }
    }

    fn codegen_field_ptr(
        &mut self,
        obj: &Expr,
        field: &str,
    ) -> Result<(PointerValue<'ctx>, Type), String> {
        let obj_ty = self.infer_expr_type(obj)?;
        let struct_name = match obj_ty {
            Type::Named(n) => n,
            _ => return Err("field access on non-struct".into()),
        };
        let info = self
            .struct_info
            .get(&struct_name)
            .ok_or_else(|| format!("unknown struct '{struct_name}'"))?
            .clone();
        let (field_idx, field_ty) = info
            .fields
            .iter()
            .enumerate()
            .find(|(_, (n, _))| n == field)
            .map(|(i, (_, t))| (i, t.clone()))
            .ok_or_else(|| format!("unknown field '{field}'"))?;

        let base_ptr = self.codegen_lvalue(obj)?;
        let field_ptr = unsafe {
            self.builder
                .build_struct_gep(base_ptr, field_idx as u32, "fieldptr")
                .map_err(|_| "gep failed".to_string())?
        };
        Ok((field_ptr, field_ty))
    }

    fn codegen_lvalue(&mut self, expr: &Expr) -> Result<PointerValue<'ctx>, String> {
        match expr {
            Expr::Ident(name, _) => self.lookup_var(name),
            Expr::FieldAccess { obj, field, .. } => {
                let (ptr, _) = self.codegen_field_ptr(obj, field)?;
                Ok(ptr)
            }
            _ => {
                let val = self.codegen_expr(expr, None)?;
                let ty = val.ty;
                let v = val.value.ok_or_else(|| "lvalue expects value".to_string())?;
                let alloca = self.create_entry_alloca("tmp", &ty)?;
                self.builder.build_store(alloca, v);
                Ok(alloca)
            }
        }
    }

    fn push_scope(&mut self) {
        self.var_scopes.push(HashMap::new());
        self.type_scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.var_scopes.pop();
        self.type_scopes.pop();
    }

    fn bind_var(&mut self, name: &str, ty: Type, ptr: PointerValue<'ctx>) {
        if let Some(scope) = self.var_scopes.last_mut() {
            scope.insert(name.to_string(), ptr);
        }
        if let Some(scope) = self.type_scopes.last_mut() {
            scope.insert(name.to_string(), ty);
        }
    }

    fn lookup_var(&self, name: &str) -> Result<PointerValue<'ctx>, String> {
        for scope in self.var_scopes.iter().rev() {
            if let Some(ptr) = scope.get(name) {
                return Ok(*ptr);
            }
        }
        Err(format!("undefined variable '{name}'"))
    }

    fn lookup_var_type(&self, name: &str) -> Result<Type, String> {
        for scope in self.type_scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Ok(ty.clone());
            }
        }
        Err(format!("undefined variable '{name}'"))
    }

    fn create_entry_alloca(&self, name: &str, ty: &Type) -> Result<PointerValue<'ctx>, String> {
        let func = self
            .current_fn
            .ok_or_else(|| "alloca outside function".to_string())?;
        let entry = func
            .get_first_basic_block()
            .ok_or_else(|| "missing entry block".to_string())?;
        let builder = self.context.create_builder();
        match entry.get_first_instruction() {
            Some(inst) => builder.position_before(&inst),
            None => builder.position_at_end(entry),
        }
        let llvm_ty = self.llvm_type(ty);
        Ok(builder.build_alloca(llvm_ty, name))
    }

    fn infer_expr_type(&self, expr: &Expr) -> Result<Type, String> {
        match expr {
            Expr::Int(_, _) => Ok(Type::I32),
            Expr::Float(_, _) => Ok(Type::F64),
            Expr::Bool(_, _) => Ok(Type::Bool),
            Expr::Str(_, _) => Ok(Type::Str),
            Expr::None(_) => Ok(Type::Optional(Box::new(Type::Void))),
            Expr::Tuple(elems, _) => {
                let mut tys = Vec::new();
                for e in elems {
                    tys.push(self.infer_expr_type(e)?);
                }
                Ok(Type::Tuple(tys))
            }
            Expr::Range { .. } => Ok(Type::Range),
            Expr::Ident(name, _) => self.lookup_var_type(name),
            Expr::StructLit { name, .. } => Ok(Type::Named(name.clone())),
            Expr::FieldAccess { obj, field, .. } => {
                let obj_ty = self.infer_expr_type(obj)?;
                let struct_name = match obj_ty {
                    Type::Named(n) => n,
                    _ => return Err("field access on non-struct".into()),
                };
                let info = self
                    .struct_info
                    .get(&struct_name)
                    .ok_or_else(|| format!("unknown struct '{struct_name}'"))?;
                for (fname, fty) in &info.fields {
                    if fname == field {
                        return Ok(fty.clone());
                    }
                }
                Err(format!("unknown field '{field}'"))
            }
            Expr::BinOp { op, left, right, .. } => {
                let lt = self.infer_expr_type(left)?;
                let rt = self.infer_expr_type(right)?;
                if lt != rt {
                    return Err("binop type mismatch".into());
                }
                Ok(self.infer_binop_type(*op, &lt))
            }
            Expr::UnaryOp { operand, .. } => self.infer_expr_type(operand),
            Expr::Call { name, .. } => self
                .sigs
                .get(name)
                .map(|s| s.ret.clone())
                .ok_or_else(|| format!("unknown function '{name}'")),
            Expr::MethodCall { obj, method, .. } => {
                let obj_ty = self.infer_expr_type(obj)?;
                let struct_name = match obj_ty {
                    Type::Named(n) => n,
                    _ => return Err("method call on non-struct".into()),
                };
                let info = self
                    .struct_info
                    .get(&struct_name)
                    .ok_or_else(|| format!("unknown struct '{struct_name}'"))?;
                info.methods
                    .get(method)
                    .map(|s| s.ret.clone())
                    .ok_or_else(|| format!("unknown method '{method}'"))
            }
            Expr::Cast { ty, .. } => Ok(ty.clone()),
            Expr::If { .. } | Expr::Match { .. } | Expr::Array(_, _) | Expr::Index { .. } => {
                Err("expression type not implemented yet".into())
            }
        }
    }

    fn infer_binop_type(&self, op: BinOp, left: &Type) -> Type {
        match op {
            BinOp::Eq
            | BinOp::NotEq
            | BinOp::Lt
            | BinOp::Gt
            | BinOp::LtEq
            | BinOp::GtEq
            | BinOp::And
            | BinOp::Or => Type::Bool,
            _ => left.clone(),
        }
    }

    fn llvm_type(&self, ty: &Type) -> BasicTypeEnum<'ctx> {
        self.llvm_type_with_self(ty, None)
    }

    fn llvm_type_with_self(&self, ty: &Type, self_type: Option<&str>) -> BasicTypeEnum<'ctx> {
        match ty {
            Type::I32 => self.context.i32_type().into(),
            Type::I64 => self.context.i64_type().into(),
            Type::F64 => self.context.f64_type().into(),
            Type::Bool => self.context.bool_type().into(),
            Type::Str => self
                .context
                .i8_type()
                .ptr_type(AddressSpace::default())
                .into(),
            Type::Range => {
                let i32t = self.context.i32_type();
                let i1t = self.context.bool_type();
                self.context
                    .struct_type(&[i32t.into(), i32t.into(), i1t.into()], false)
                    .into()
            }
            Type::Tuple(elems) => {
                let tys: Vec<BasicTypeEnum<'ctx>> =
                    elems.iter().map(|t| self.llvm_type(t)).collect();
                self.context.struct_type(&tys, false).into()
            }
            Type::Array(inner) => {
                let elem = self.llvm_type(inner);
                elem.ptr_type(AddressSpace::default()).into()
            }
            Type::Optional(inner) => {
                let inner_ty = self.llvm_type(inner);
                let i1t = self.context.bool_type();
                self.context
                    .struct_type(&[i1t.into(), inner_ty], false)
                    .into()
            }
            Type::Named(name) => self
                .structs
                .get(name)
                .copied()
                .unwrap_or_else(|| self.context.opaque_struct_type(name))
                .into(),
            Type::SelfType => {
                let name = self_type.unwrap_or("self");
                self.structs
                    .get(name)
                    .copied()
                    .unwrap_or_else(|| self.context.opaque_struct_type(name))
                    .into()
            }
            Type::Void => self.context.i8_type().into(),
        }
    }

}
