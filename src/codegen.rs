use std::collections::HashMap;
use std::path::Path;

use inkwell::builder::Builder;
use inkwell::basic_block::BasicBlock;
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
    break_stack: Vec<BasicBlock<'ctx>>,
    continue_stack: Vec<BasicBlock<'ctx>>,
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
            break_stack: vec![],
            continue_stack: vec![],
        }
    }

    pub fn module(&self) -> &Module<'ctx> {
        &self.module
    }

    pub fn compile(&mut self, items: &[Item]) -> Result<(), String> {
        self.collect_structs(items)?;
        self.collect_signatures(items)?;
        self.declare_functions(items)?;
        self.declare_runtime()?;
        self.define_items(items)?;
        Ok(())
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

        let streq = self
            .context
            .bool_type()
            .fn_type(&[i8ptr.into(), i8ptr.into()], false);
        self.module.add_function("pebbles_str_eq", streq, None);
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
                let inferred = if ty.is_none() {
                    Some(self.infer_expr_type(value)?)
                } else {
                    None
                };
                let expected = ty.as_ref().or(inferred.as_ref());
                let rhs = self.codegen_expr(value, expected)?;
                let var_ty = ty.clone().or(inferred).unwrap_or(rhs.ty.clone());
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
            Stmt::While { cond, body, .. } => self.codegen_while(cond, body),
            Stmt::For { var, iter, body, .. } => self.codegen_for(var, iter, body),
            Stmt::Break { .. } => {
                let target = *self
                    .break_stack
                    .last()
                    .ok_or_else(|| "break outside loop".to_string())?;
                self.builder.build_unconditional_branch(target);
                Ok(())
            }
            Stmt::Continue { .. } => {
                let target = *self
                    .continue_stack
                    .last()
                    .ok_or_else(|| "continue outside loop".to_string())?;
                self.builder.build_unconditional_branch(target);
                Ok(())
            }
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
                if !matches!(opt_ty, Type::Optional(_)) {
                    return Err("none requires optional type".into());
                }
                let llvm_ty = self.llvm_type(&opt_ty).into_struct_type();
                let mut val = llvm_ty.get_undef();
                let zero = self.context.bool_type().const_int(0, false);
                val = self
                    .builder
                    .build_insert_value(val, zero, 0, "none")
                    .unwrap()
                    .into_struct_value();
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
                if name == "print" {
                    let arg = args.first().ok_or_else(|| "print expects 1 arg".to_string())?;
                    let v = self.codegen_expr(arg, Some(&Type::Str))?;
                    let val = v.value.ok_or_else(|| "print expects value".to_string())?;
                    let f = self
                        .module
                        .get_function("pebbles_print_str")
                        .ok_or_else(|| "missing pebbles_print_str".to_string())?;
                    self.builder.build_call(f, &[val], "print");
                    return Ok(CgValue { value: None, ty: Type::Void });
                }
                if name == "input" {
                    let f = self
                        .module
                        .get_function("pebbles_input")
                        .ok_or_else(|| "missing pebbles_input".to_string())?;
                    let call = self.builder.build_call(f, &[], "input");
                    let v = call.try_as_basic_value().left().unwrap();
                    return Ok(CgValue { value: Some(v), ty: Type::Str });
                }
                if name == "len" {
                    let arg = args.first().ok_or_else(|| "len expects 1 arg".to_string())?;
                    let v = self.codegen_expr(arg, Some(&Type::Str))?;
                    let val = v.value.ok_or_else(|| "len expects value".to_string())?;
                    let f = self
                        .module
                        .get_function("pebbles_len_str")
                        .ok_or_else(|| "missing pebbles_len_str".to_string())?;
                    let call = self.builder.build_call(f, &[val], "len");
                    let v = call.try_as_basic_value().left().unwrap();
                    return Ok(CgValue { value: Some(v), ty: Type::I32 });
                }
                if name == "str" {
                    let arg = args.first().ok_or_else(|| "str expects 1 arg".to_string())?;
                    let arg_ty = self.infer_expr_type(arg)?;
                    let v = self.codegen_expr(arg, Some(&arg_ty))?;
                    let val = v.value.ok_or_else(|| "str expects value".to_string())?;
                    if arg_ty == Type::Str {
                        return Ok(CgValue { value: Some(val), ty: Type::Str });
                    }
                    if arg_ty == Type::I32 {
                        let f = self
                            .module
                            .get_function("pebbles_str_i32")
                            .ok_or_else(|| "missing pebbles_str_i32".to_string())?;
                        let call = self.builder.build_call(f, &[val], "str");
                        let v = call.try_as_basic_value().left().unwrap();
                        return Ok(CgValue { value: Some(v), ty: Type::Str });
                    }
                    return Err("str() only supports i32 and str for now".into());
                }
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
            Expr::If { cond, then, else_, .. } => {
                let inferred = if expected.is_none() {
                    Some(self.infer_expr_type(expr)?)
                } else {
                    None
                };
                let exp = expected.or(inferred.as_ref());
                self.codegen_if_expr(cond, then, else_, exp)
            }
            Expr::Match { subject, arms, .. } => {
                let inferred = if expected.is_none() {
                    Some(self.infer_expr_type(expr)?)
                } else {
                    None
                };
                let exp = expected.or(inferred.as_ref());
                self.codegen_match_expr(subject, arms, exp)
            }
            Expr::Array(_, _) | Expr::Index { .. } => Err("expression not codegenned yet".into()),
        }
    }

    fn codegen_if_expr(
        &mut self,
        cond: &Expr,
        then: &[Stmt],
        else_: &Option<Vec<Stmt>>,
        expected: Option<&Type>,
    ) -> Result<CgValue<'ctx>, String> {
        if expected.is_some()
            && expected != Some(&Type::Void)
            && else_.is_none()
        {
            return Err("if expression with value requires else".into());
        }
        let cond_val = self.codegen_expr(cond, Some(&Type::Bool))?;
        let cond_v = cond_val
            .value
            .ok_or_else(|| "if condition expects value".to_string())?
            .into_int_value();

        let func = self
            .current_fn
            .ok_or_else(|| "if outside function".to_string())?;
        let then_bb = self.context.append_basic_block(func, "then");
        let else_bb = self.context.append_basic_block(func, "else");
        let merge_bb = self.context.append_basic_block(func, "ifend");

        self.builder
            .build_conditional_branch(cond_v, then_bb, else_bb);

        self.builder.position_at_end(then_bb);
        let then_val = self.codegen_block(then, expected)?;
        if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
            self.builder.build_unconditional_branch(merge_bb);
        }
        let then_end = self.builder.get_insert_block().unwrap();

        self.builder.position_at_end(else_bb);
        let else_val = match else_ {
            Some(stmts) => self.codegen_block(stmts, expected)?,
            None => None,
        };
        if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
            self.builder.build_unconditional_branch(merge_bb);
        }
        let else_end = self.builder.get_insert_block().unwrap();

        self.builder.position_at_end(merge_bb);

        let ty = expected.cloned().unwrap_or(Type::Void);
        if ty == Type::Void {
            return Ok(CgValue { value: None, ty });
        }
        let llvm_ty = self.llvm_type(&ty);
        let phi = self.builder.build_phi(llvm_ty, "iftmp");
        if let Some(v) = then_val {
            phi.add_incoming(&[(&v, then_end)]);
        }
        if let Some(v) = else_val {
            phi.add_incoming(&[(&v, else_end)]);
        }
        Ok(CgValue {
            value: Some(phi.as_basic_value()),
            ty,
        })
    }

    fn codegen_match_expr(
        &mut self,
        subject: &Expr,
        arms: &[MatchArm],
        expected: Option<&Type>,
    ) -> Result<CgValue<'ctx>, String> {
        let subj_val = self.codegen_expr(subject, None)?;
        let subj_v = subj_val
            .value
            .ok_or_else(|| "match subject expects value".to_string())?;
        let subj_ty = subj_val.ty.clone();

        let func = self
            .current_fn
            .ok_or_else(|| "match outside function".to_string())?;
        let end_bb = self.context.append_basic_block(func, "match.end");
        let mut check_bb = self.context.append_basic_block(func, "match.check");

        self.builder.build_unconditional_branch(check_bb);
        self.builder.position_at_end(check_bb);

        let mut incoming: Vec<(BasicValueEnum<'ctx>, BasicBlock<'ctx>)> = Vec::new();
        let exp_ty = expected.cloned().unwrap_or(Type::Void);

        for (idx, arm) in arms.iter().enumerate() {
            let is_last = idx + 1 == arms.len();
            let arm_bb = self.context.append_basic_block(func, "match.arm");
            let next_bb = if is_last {
                end_bb
            } else {
                self.context.append_basic_block(func, "match.next")
            };

            let (cond, bindings) =
                self.codegen_pattern_cond(&arm.pattern, subj_v, &subj_ty)?;

            if is_last {
                self.builder
                    .build_conditional_branch(cond, arm_bb, end_bb);
            } else {
                self.builder
                    .build_conditional_branch(cond, arm_bb, next_bb);
            }

            self.builder.position_at_end(arm_bb);
            self.push_scope();
            for (name, val, ty) in bindings {
                let alloca = self.create_entry_alloca(&name, &ty)?;
                self.builder.build_store(alloca, val);
                self.bind_var(&name, ty, alloca);
            }

            let arm_val = self.codegen_block(&arm.body, expected)?;
            self.pop_scope();

            if exp_ty != Type::Void {
                let v = arm_val.ok_or_else(|| "match arm missing value".to_string())?;
                incoming.push((v, self.builder.get_insert_block().unwrap()));
            }

            if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
                self.builder.build_unconditional_branch(end_bb);
            }

            check_bb = next_bb;
            if !is_last {
                self.builder.position_at_end(check_bb);
            }
        }

        self.builder.position_at_end(end_bb);
        if exp_ty == Type::Void {
            return Ok(CgValue { value: None, ty: exp_ty });
        }
        let llvm_ty = self.llvm_type(&exp_ty);
        let phi = self.builder.build_phi(llvm_ty, "matchtmp");
        for (v, b) in incoming {
            phi.add_incoming(&[(&v, b)]);
        }
        Ok(CgValue {
            value: Some(phi.as_basic_value()),
            ty: exp_ty,
        })
    }

    fn codegen_pattern_cond(
        &mut self,
        pattern: &Pattern,
        value: BasicValueEnum<'ctx>,
        ty: &Type,
    ) -> Result<(inkwell::values::IntValue<'ctx>, Vec<(String, BasicValueEnum<'ctx>, Type)>), String>
    {
        let true_val = self.context.bool_type().const_int(1, false);
        match pattern {
            Pattern::Wildcard => Ok((true_val, vec![])),
            Pattern::Binding(name) => Ok((true_val, vec![(name.clone(), value, ty.clone())])),
            Pattern::Int(n) => {
                let v = value.into_int_value();
                let c = self.context.i32_type().const_int(*n as u64, true);
                Ok((self.builder.build_int_compare(inkwell::IntPredicate::EQ, v, c, "m_eq"), vec![]))
            }
            Pattern::Bool(b) => {
                let v = value.into_int_value();
                let c = self.context.bool_type().const_int(u64::from(*b), false);
                Ok((self.builder.build_int_compare(inkwell::IntPredicate::EQ, v, c, "m_eq"), vec![]))
            }
            Pattern::Float(f) => {
                let v = value.into_float_value();
                let c = self.context.f64_type().const_float(*f);
                Ok((self.builder.build_float_compare(inkwell::FloatPredicate::OEQ, v, c, "m_eq"), vec![]))
            }
            Pattern::Str(s) => {
                let f = self
                    .module
                    .get_function("pebbles_str_eq")
                    .ok_or_else(|| "missing pebbles_str_eq".to_string())?;
                let lit = self
                    .builder
                    .build_global_string_ptr(s, "mstr")
                    .as_pointer_value()
                    .into();
                let call = self.builder.build_call(f, &[value, lit], "streq");
                Ok((call.try_as_basic_value().left().unwrap().into_int_value(), vec![]))
            }
            Pattern::None => {
                if let Type::Optional(_) = ty {
                    let opt = value.into_struct_value();
                    let is_some = self
                        .builder
                        .build_extract_value(opt, 0, "is_some")
                        .unwrap()
                        .into_int_value();
                    let zero = self.context.bool_type().const_int(0, false);
                    Ok((self.builder.build_int_compare(inkwell::IntPredicate::EQ, is_some, zero, "isnone"), vec![]))
                } else {
                    Err("none pattern on non-optional".into())
                }
            }
            Pattern::Tuple(pats) => {
                let tup = value.into_struct_value();
                let mut cond = true_val;
                let mut binds = Vec::new();
                let mut elem_types = match ty {
                    Type::Tuple(ts) => ts.clone(),
                    _ => return Err("tuple pattern on non-tuple".into()),
                };
                for (idx, pat) in pats.iter().enumerate() {
                    let elem = self
                        .builder
                        .build_extract_value(tup, idx as u32, "te")
                        .unwrap();
                    let t = elem_types.remove(0);
                    let (c, mut b) = self.codegen_pattern_cond(pat, elem, &t)?;
                    cond = self.builder.build_and(cond, c, "m_and");
                    binds.append(&mut b);
                }
                Ok((cond, binds))
            }
            Pattern::Struct { name, fields } => {
                let st = value.into_struct_value();
                let info = self
                    .struct_info
                    .get(name)
                    .ok_or_else(|| format!("unknown struct '{name}'"))?;
                let mut cond = true_val;
                let mut binds = Vec::new();
                for (field_name, pat) in fields {
                    let (idx, fty) = info
                        .fields
                        .iter()
                        .enumerate()
                        .find(|(_, (n, _))| n == field_name)
                        .map(|(i, (_, t))| (i, t.clone()))
                        .ok_or_else(|| format!("unknown field '{field_name}'"))?;
                    let field_val = self
                        .builder
                        .build_extract_value(st, idx as u32, "sf")
                        .unwrap();
                    let (c, mut b) = self.codegen_pattern_cond(pat, field_val, &fty)?;
                    cond = self.builder.build_and(cond, c, "m_and");
                    binds.append(&mut b);
                }
                Ok((cond, binds))
            }
        }
    }

    fn codegen_block(
        &mut self,
        stmts: &[Stmt],
        expected: Option<&Type>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        self.push_scope();
        let mut result = None;
        for (idx, stmt) in stmts.iter().enumerate() {
            let last = idx + 1 == stmts.len();
            if last {
                if let Stmt::Expr(expr) = stmt {
                    if expected.is_some() && expected != Some(&Type::Void) {
                        let v = self.codegen_expr(expr, expected)?;
                        result = v.value;
                        continue;
                    }
                }
            }
            self.codegen_stmt(stmt)?;
            if let Some(block) = self.builder.get_insert_block() {
                if block.get_terminator().is_some() {
                    break;
                }
            }
        }
        self.pop_scope();
        Ok(result)
    }

    fn codegen_while(&mut self, cond: &Expr, body: &[Stmt]) -> Result<(), String> {
        let func = self
            .current_fn
            .ok_or_else(|| "while outside function".to_string())?;
        let cond_bb = self.context.append_basic_block(func, "while.cond");
        let body_bb = self.context.append_basic_block(func, "while.body");
        let end_bb = self.context.append_basic_block(func, "while.end");

        self.builder.build_unconditional_branch(cond_bb);
        self.builder.position_at_end(cond_bb);
        let cond_val = self.codegen_expr(cond, Some(&Type::Bool))?;
        let cond_v = cond_val
            .value
            .ok_or_else(|| "while condition expects value".to_string())?
            .into_int_value();
        self.builder
            .build_conditional_branch(cond_v, body_bb, end_bb);

        self.builder.position_at_end(body_bb);
        self.break_stack.push(end_bb);
        self.continue_stack.push(cond_bb);
        self.codegen_block(body, None)?;
        self.break_stack.pop();
        self.continue_stack.pop();
        if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
            self.builder.build_unconditional_branch(cond_bb);
        }

        self.builder.position_at_end(end_bb);
        Ok(())
    }

    fn codegen_for(&mut self, var: &str, iter: &Expr, body: &[Stmt]) -> Result<(), String> {
        let iter_ty = self.infer_expr_type(iter)?;
        if iter_ty != Type::Range {
            return Err("for loop only supports range for now".into());
        }
        let func = self
            .current_fn
            .ok_or_else(|| "for outside function".to_string())?;

        let range_val = self.codegen_expr(iter, Some(&Type::Range))?;
        let range = range_val
            .value
            .ok_or_else(|| "for range expects value".to_string())?;
        let start = self
            .builder
            .build_extract_value(range.into_struct_value(), 0, "start")
            .unwrap()
            .into_int_value();
        let end = self
            .builder
            .build_extract_value(range.into_struct_value(), 1, "end")
            .unwrap()
            .into_int_value();
        let inclusive = self
            .builder
            .build_extract_value(range.into_struct_value(), 2, "incl")
            .unwrap()
            .into_int_value();

        let idx_alloca = self.create_entry_alloca(var, &Type::I32)?;
        self.builder.build_store(idx_alloca, start);
        self.push_scope();
        self.bind_var(var, Type::I32, idx_alloca);

        let cond_bb = self.context.append_basic_block(func, "for.cond");
        let body_bb = self.context.append_basic_block(func, "for.body");
        let incr_bb = self.context.append_basic_block(func, "for.incr");
        let end_bb = self.context.append_basic_block(func, "for.end");

        self.builder.build_unconditional_branch(cond_bb);
        self.builder.position_at_end(cond_bb);
        let cur = self.builder.build_load(idx_alloca, "i").into_int_value();
        let cmp = self.builder.build_select(
            inclusive,
            self.builder
                .build_int_compare(inkwell::IntPredicate::SLE, cur, end, "cmple")
                .into(),
            self.builder
                .build_int_compare(inkwell::IntPredicate::SLT, cur, end, "cmplt")
                .into(),
            "cmp",
        );
        let cmp_i1 = cmp.into_int_value();
        self.builder
            .build_conditional_branch(cmp_i1, body_bb, end_bb);

        self.builder.position_at_end(body_bb);
        self.break_stack.push(end_bb);
        self.continue_stack.push(incr_bb);
        self.codegen_block(body, None)?;
        self.break_stack.pop();
        self.continue_stack.pop();
        if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
            self.builder.build_unconditional_branch(incr_bb);
        }

        self.builder.position_at_end(incr_bb);
        let cur = self.builder.build_load(idx_alloca, "i").into_int_value();
        let next = self.builder.build_int_add(
            cur,
            self.context.i32_type().const_int(1, false),
            "i.next",
        );
        self.builder.build_store(idx_alloca, next);
        self.builder.build_unconditional_branch(cond_bb);

        self.builder.position_at_end(end_bb);
        self.pop_scope();
        Ok(())
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
                if *ty == Type::Str && (op == BinOp::Eq || op == BinOp::NotEq) {
                    let f = self
                        .module
                        .get_function("pebbles_str_eq")
                        .ok_or_else(|| "missing pebbles_str_eq".to_string())?;
                    let call = self.builder.build_call(f, &[l, r], "streq");
                    let mut v = call.try_as_basic_value().left().unwrap().into_int_value();
                    if op == BinOp::NotEq {
                        v = self.builder.build_not(v, "not");
                    }
                    Ok(v.into())
                } else if *ty == Type::F64 {
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
                .or_else(|| {
                    if name == "str" {
                        Some(Type::Str)
                    } else {
                        None
                    }
                })
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
            Expr::If { then, else_, .. } => {
                let then_ty = self.infer_block_tail_type(then)?;
                match else_ {
                    None => Ok(Type::Void),
                    Some(stmts) => {
                        let else_ty = self.infer_block_tail_type(stmts)?;
                        match (then_ty, else_ty) {
                            (Some(t), Some(e)) if t == e => Ok(t),
                            _ => Ok(Type::Void),
                        }
                    }
                }
            }
            Expr::Match { arms, .. } => {
                let mut result: Option<Type> = None;
                for arm in arms {
                    let arm_ty = self.infer_block_tail_type(&arm.body)?;
                    match (&result, arm_ty) {
                        (None, Some(t)) => result = Some(t),
                        (Some(existing), Some(t)) if *existing == t => {}
                        _ => {}
                    }
                }
                Ok(result.unwrap_or(Type::Void))
            }
            Expr::Array(_, _) | Expr::Index { .. } => {
                Err("expression type not implemented yet".into())
            }
        }
    }

    fn infer_block_tail_type(&self, stmts: &[Stmt]) -> Result<Option<Type>, String> {
        match stmts.last() {
            Some(Stmt::Expr(e)) => Ok(Some(self.infer_expr_type(e)?)),
            _ => Ok(None),
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
