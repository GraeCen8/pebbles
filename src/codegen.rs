use std::collections::HashMap;
use std::path::Path;

use inkwell::builder::Builder;
use inkwell::basic_block::BasicBlock;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::{InitializationConfig, Target, TargetMachine};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, StructType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, PointerValue, ValueKind,
};
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
    array_ty: StructType<'ctx>,
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
        let i32t = context.i32_type();
        let i8ptr = context
            .i8_type()
            .ptr_type(AddressSpace::default());
        let array_ty = context.struct_type(
            &[
                i32t.into(), // len
                i32t.into(), // cap
                i8ptr.into(), // data
                i32t.into(), // elem_size
            ],
            false,
        );
        Self {
            context,
            module,
            builder,
            structs: HashMap::new(),
            array_ty,
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

    fn b<T>(&self, res: Result<T, inkwell::builder::BuilderError>) -> Result<T, String> {
        res.map_err(|e| e.to_string())
    }

    fn call_value(
        &self,
        call: inkwell::values::CallSiteValue<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        match call.try_as_basic_value() {
            ValueKind::Basic(v) => Some(v),
            _ => None,
        }
    }

    fn elem_size_value(&self, elem_ty: &Type) -> Result<IntValue<'ctx>, String> {
        let llvm_ty = self.llvm_type(elem_ty);
        let size = llvm_ty
            .size_of()
            .ok_or_else(|| "size_of failed".to_string())?;
        Ok(self.b(self.builder.build_int_cast(
            size,
            self.context.i32_type(),
            "elem_size",
        ))?)
    }

    fn array_len_from_value(&self, array_val: BasicValueEnum<'ctx>) -> Result<IntValue<'ctx>, String> {
        let v = array_val
            .into_struct_value();
        let len = self
            .b(self.builder.build_extract_value(v, 0, "len"))?
            .into_int_value();
        Ok(len)
    }

    fn array_data_ptr_from_value(
        &self,
        array_val: BasicValueEnum<'ctx>,
    ) -> Result<PointerValue<'ctx>, String> {
        let v = array_val.into_struct_value();
        let data = self
            .b(self.builder.build_extract_value(v, 2, "data"))?
            .into_pointer_value();
        Ok(data)
    }

    fn array_elem_ptr_from_value(
        &self,
        array_val: BasicValueEnum<'ctx>,
        idx: IntValue<'ctx>,
        elem_ty: &Type,
    ) -> Result<PointerValue<'ctx>, String> {
        let data = self.array_data_ptr_from_value(array_val)?;
        let elem_size = self.elem_size_value(elem_ty)?;
        let offset = self
            .b(self.builder.build_int_mul(idx, elem_size, "ofs"))?;
        let ptr = unsafe {
            self.b(self.builder.build_gep(
                self.context.i8_type(),
                data,
                &[offset],
                "elem_ptr",
            ))?
        };
        let elem_ptr = self
            .b(self.builder.build_bit_cast(
                ptr,
                self.llvm_type(elem_ty).ptr_type(AddressSpace::default()),
                "elem_cast",
            ))?
            .into_pointer_value();
        Ok(elem_ptr)
    }

    fn compare_values(
        &self,
        ty: &Type,
        l: BasicValueEnum<'ctx>,
        r: BasicValueEnum<'ctx>,
    ) -> Result<IntValue<'ctx>, String> {
        match ty {
            Type::I32 | Type::I64 | Type::Bool => Ok(self
                .b(self.builder.build_int_compare(
                    inkwell::IntPredicate::EQ,
                    l.into_int_value(),
                    r.into_int_value(),
                    "eq",
                ))?),
            Type::F64 => Ok(self
                .b(self.builder.build_float_compare(
                    inkwell::FloatPredicate::OEQ,
                    l.into_float_value(),
                    r.into_float_value(),
                    "feq",
                ))?),
            Type::Str => {
                let f = self
                    .module
                    .get_function("pebbles_str_eq")
                    .ok_or_else(|| "missing pebbles_str_eq".to_string())?;
                let call = self.b(self.builder.build_call(
                    f,
                    &[
                        BasicMetadataValueEnum::from(l),
                        BasicMetadataValueEnum::from(r),
                    ],
                    "streq",
                ))?;
                Ok(self
                    .call_value(call)
                    .ok_or_else(|| "streq returned void".to_string())?
                    .into_int_value())
            }
            Type::Range => {
                let lv = l.into_struct_value();
                let rv = r.into_struct_value();
                let mut cur = self.context.bool_type().const_int(1, false);
                for i in 0..3 {
                    let le = self
                        .b(self.builder.build_extract_value(lv, i as u32, "le"))?
                        .into_int_value();
                    let re = self
                        .b(self.builder.build_extract_value(rv, i as u32, "re"))?
                        .into_int_value();
                    let eq = self.b(self.builder.build_int_compare(
                        inkwell::IntPredicate::EQ,
                        le,
                        re,
                        "eq",
                    ))?;
                    cur = self.b(self.builder.build_and(cur, eq, "and"))?;
                }
                Ok(cur)
            }
            Type::Tuple(elems) => {
                let lv = l.into_struct_value();
                let rv = r.into_struct_value();
                let mut cur = self.context.bool_type().const_int(1, false);
                for (idx, elem_ty) in elems.iter().enumerate() {
                    let le = self
                        .b(self.builder.build_extract_value(lv, idx as u32, "le"))?;
                    let re = self
                        .b(self.builder.build_extract_value(rv, idx as u32, "re"))?;
                    let eq = self.compare_values(elem_ty, le, re)?;
                    cur = self.b(self.builder.build_and(cur, eq, "and"))?;
                }
                Ok(cur)
            }
            Type::Optional(inner) => {
                let lv = l.into_struct_value();
                let rv = r.into_struct_value();
                let ls = self
                    .b(self.builder.build_extract_value(lv, 0, "ls"))?
                    .into_int_value();
                let rs = self
                    .b(self.builder.build_extract_value(rv, 0, "rs"))?
                    .into_int_value();
                let both_some = self.b(self.builder.build_and(ls, rs, "both"))?;
                let ls_not = self.b(self.builder.build_not(ls, "ls_not"))?;
                let rs_not = self.b(self.builder.build_not(rs, "rs_not"))?;
                let none_none = self.b(self.builder.build_and(ls_not, rs_not, "none"))?;
                let inner_eq = if **inner == Type::Void {
                    self.context.bool_type().const_int(1, false)
                } else {
                    let le = self
                        .b(self.builder.build_extract_value(lv, 1, "le"))?;
                    let re = self
                        .b(self.builder.build_extract_value(rv, 1, "re"))?;
                    self.compare_values(inner, le, re)?
                };
                let both_some_eq = self.b(self.builder.build_and(both_some, inner_eq, "both_eq"))?;
                Ok(self.b(self.builder.build_or(none_none, both_some_eq, "opt_eq"))?)
            }
            Type::Named(name) => {
                let info = self
                    .struct_info
                    .get(name)
                    .ok_or_else(|| format!("unknown struct '{name}'"))?
                    .clone();
                let lv = l.into_struct_value();
                let rv = r.into_struct_value();
                let mut cur = self.context.bool_type().const_int(1, false);
                for (idx, (_, fty)) in info.fields.iter().enumerate() {
                    let le = self
                        .b(self.builder.build_extract_value(lv, idx as u32, "le"))?;
                    let re = self
                        .b(self.builder.build_extract_value(rv, idx as u32, "re"))?;
                    let eq = self.compare_values(fty, le, re)?;
                    cur = self.b(self.builder.build_and(cur, eq, "and"))?;
                }
                Ok(cur)
            }
            Type::Array(inner) => {
                let func = self
                    .current_fn
                    .ok_or_else(|| "array equality outside function".to_string())?;
                let l_len = self.array_len_from_value(l)?;
                let r_len = self.array_len_from_value(r)?;
                let len_eq = self.b(self.builder.build_int_compare(
                    inkwell::IntPredicate::EQ,
                    l_len,
                    r_len,
                    "len_eq",
                ))?;

                let result_alloca = self.create_entry_alloca("arr_eq", &Type::Bool)?;
                let idx_alloca = self.create_entry_alloca("arr_idx", &Type::I32)?;

                let len_ok_bb = self.context.append_basic_block(func, "arr_eq.len_ok");
                let len_fail_bb = self.context.append_basic_block(func, "arr_eq.len_fail");
                let cond_bb = self.context.append_basic_block(func, "arr_eq.cond");
                let body_bb = self.context.append_basic_block(func, "arr_eq.body");
                let end_bb = self.context.append_basic_block(func, "arr_eq.end");

                self.b(self.builder.build_conditional_branch(len_eq, len_ok_bb, len_fail_bb))?;

                self.builder.position_at_end(len_fail_bb);
                self.b(self.builder.build_store(
                    result_alloca,
                    self.context.bool_type().const_int(0, false),
                ))?;
                self.b(self.builder.build_unconditional_branch(end_bb))?;

                self.builder.position_at_end(len_ok_bb);
                self.b(self.builder.build_store(
                    result_alloca,
                    self.context.bool_type().const_int(1, false),
                ))?;
                self.b(self.builder.build_store(
                    idx_alloca,
                    self.context.i32_type().const_int(0, false),
                ))?;
                self.b(self.builder.build_unconditional_branch(cond_bb))?;

                self.builder.position_at_end(cond_bb);
                let cur = self
                    .b(self.builder.build_load(self.context.i32_type(), idx_alloca, "i"))?
                    .into_int_value();
                let cmp = self.b(self.builder.build_int_compare(
                    inkwell::IntPredicate::SLT,
                    cur,
                    l_len,
                    "cmp",
                ))?;
                self.b(self.builder.build_conditional_branch(cmp, body_bb, end_bb))?;

                self.builder.position_at_end(body_bb);
                let l_ptr = self.array_elem_ptr_from_value(l, cur, inner)?;
                let r_ptr = self.array_elem_ptr_from_value(r, cur, inner)?;
                let l_val = self.b(self.builder.build_load(self.llvm_type(inner), l_ptr, "l"))?;
                let r_val = self.b(self.builder.build_load(self.llvm_type(inner), r_ptr, "r"))?;
                let eq = self.compare_values(inner, l_val, r_val)?;
                let is_eq = self.b(self.builder.build_int_compare(
                    inkwell::IntPredicate::EQ,
                    eq,
                    self.context.bool_type().const_int(1, false),
                    "is_eq",
                ))?;
                let fail_bb = self.context.append_basic_block(func, "arr_eq.fail");
                let cont_bb = self.context.append_basic_block(func, "arr_eq.cont");
                self.b(self.builder.build_conditional_branch(is_eq, cont_bb, fail_bb))?;

                self.builder.position_at_end(fail_bb);
                self.b(self.builder.build_store(
                    result_alloca,
                    self.context.bool_type().const_int(0, false),
                ))?;
                self.b(self.builder.build_unconditional_branch(end_bb))?;

                self.builder.position_at_end(cont_bb);
                let next = self.b(self.builder.build_int_add(
                    cur,
                    self.context.i32_type().const_int(1, false),
                    "next",
                ))?;
                self.b(self.builder.build_store(idx_alloca, next))?;
                self.b(self.builder.build_unconditional_branch(cond_bb))?;

                self.builder.position_at_end(end_bb);
                let res = self
                    .b(self.builder.build_load(self.context.bool_type(), result_alloca, "res"))?
                    .into_int_value();
                Ok(res)
            }
            Type::Void | Type::SelfType => Err("equality not supported for this type".into()),
        }
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
        let mut param_types: Vec<BasicMetadataTypeEnum<'ctx>> = vec![];
        for p in params {
            let ty = self.llvm_type_with_self(&p.ty, self_type);
            param_types.push(ty.into());
        }
        let fn_type = match ret {
            Type::Void => self.context.void_type().fn_type(&param_types, false),
            _ => self
                .llvm_type_with_self(ret, self_type)
                .fn_type(&param_types, false),
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
            "int".into(),
            FnSig {
                params: vec![Type::Str],
                ret: Type::I32,
            },
        );
        self.sigs.insert(
            "float".into(),
            FnSig {
                params: vec![Type::Str],
                ret: Type::F64,
            },
        );
        self.sigs.insert(
            "sqrt".into(),
            FnSig {
                params: vec![Type::F64],
                ret: Type::F64,
            },
        );
    }

    fn declare_runtime(&mut self) -> Result<(), String> {
        let i8ptr = self.context.i8_type().ptr_type(AddressSpace::default());
        let i32t = self.context.i32_type();
        let i1t = self.context.bool_type();
        let voidt = self.context.void_type();

        let print = voidt.fn_type(&[BasicMetadataTypeEnum::from(i8ptr)], false);
        self.module.add_function("pebbles_print_str", print, None);

        let input = i8ptr.fn_type(&[], false);
        self.module.add_function("pebbles_input", input, None);

        let len = i32t.fn_type(&[BasicMetadataTypeEnum::from(i8ptr)], false);
        self.module.add_function("pebbles_len_str", len, None);

        let str_i32 = i8ptr.fn_type(&[BasicMetadataTypeEnum::from(i32t)], false);
        self.module.add_function("pebbles_str_i32", str_i32, None);

        let concat = i8ptr.fn_type(
            &[
                BasicMetadataTypeEnum::from(i8ptr),
                BasicMetadataTypeEnum::from(i8ptr),
            ],
            false,
        );
        self.module
            .add_function("pebbles_str_concat", concat, None);

        let streq = self
            .context
            .bool_type()
            .fn_type(
                &[
                    BasicMetadataTypeEnum::from(i8ptr),
                    BasicMetadataTypeEnum::from(i8ptr),
                ],
                false,
            );
        self.module.add_function("pebbles_str_eq", streq, None);

        let int_str = i32t.fn_type(&[BasicMetadataTypeEnum::from(i8ptr)], false);
        self.module.add_function("pebbles_int_str", int_str, None);

        let float_str = self.context.f64_type().fn_type(&[BasicMetadataTypeEnum::from(i8ptr)], false);
        self.module.add_function("pebbles_float_str", float_str, None);

        let sqrt_f64 = self.context.f64_type().fn_type(&[BasicMetadataTypeEnum::from(self.context.f64_type())], false);
        self.module.add_function("pebbles_sqrt_f64", sqrt_f64, None);

        let str_index = i8ptr.fn_type(&[BasicMetadataTypeEnum::from(i8ptr), BasicMetadataTypeEnum::from(i32t)], false);
        self.module.add_function("pebbles_str_index", str_index, None);

        let array_new = self.array_ty.fn_type(
            &[BasicMetadataTypeEnum::from(i32t), BasicMetadataTypeEnum::from(i32t)],
            false,
        );
        self.module.add_function("pebbles_array_new", array_new, None);

        let array_ptr = self.array_ty.ptr_type(AddressSpace::default());
        let array_push = voidt.fn_type(
            &[BasicMetadataTypeEnum::from(array_ptr), BasicMetadataTypeEnum::from(i8ptr)],
            false,
        );
        self.module.add_function("pebbles_array_push", array_push, None);

        let array_pop = i1t.fn_type(
            &[BasicMetadataTypeEnum::from(array_ptr), BasicMetadataTypeEnum::from(i8ptr)],
            false,
        );
        self.module.add_function("pebbles_array_pop", array_pop, None);
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
            self.b(self.builder.build_store(alloca, llvm_param))?;
            self.bind_var(&param.name, param_ty, alloca);
        }

        for stmt in &f.body {
            self.codegen_stmt(stmt)?;
        }

        if let Some(block) = self.builder.get_insert_block() {
            if block.get_terminator().is_none() {
                match &f.ret {
                    Type::Void => {
                        self.b(self.builder.build_return(None))?;
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
                    self.b(self.builder.build_store(alloca, val))?;
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
                self.b(self.builder.build_store(ptr, val))?;
                Ok(())
            }
            Stmt::Return { value, .. } => {
                let ret_ty = self.current_ret.clone().unwrap_or(Type::Void);
                let ret_val = if let Some(expr) = value {
                    let v = self.codegen_expr(expr, Some(&ret_ty))?;
                    v.value
                        .ok_or_else(|| "return expects value".to_string())?
                } else {
                    self.b(self.builder.build_return(None))?;
                    return Ok(());
                };
                self.b(self.builder.build_return(Some(&ret_val)))?;
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
                self.b(self.builder.build_unconditional_branch(target))?;
                Ok(())
            }
            Stmt::Continue { .. } => {
                let target = *self
                    .continue_stack
                    .last()
                    .ok_or_else(|| "continue outside loop".to_string())?;
                self.b(self.builder.build_unconditional_branch(target))?;
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
                    .b(self.builder.build_global_string_ptr(s, "str"))?
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
                    .b(self.builder.build_insert_value(val, zero, 0, "none"))?
                    .into_struct_value();
                Ok(CgValue {
                    value: Some(val.into()),
                    ty: opt_ty,
                })
            }
            Expr::Ident(name, _) => {
                let ptr = self.lookup_var(name)?;
                let ty = self.lookup_var_type(name)?;
                let llvm_ty = self.llvm_type(&ty);
                let val = self.b(self.builder.build_load(llvm_ty, ptr, name))?;
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
                    cur = self
                        .b(self.builder.build_insert_value(cur, *v, idx as u32, "tup"))?
                        .into_struct_value();
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
                cur = self
                    .b(self.builder.build_insert_value(cur, s_val, 0, "range"))?
                    .into_struct_value();
                cur = self
                    .b(self.builder.build_insert_value(cur, e_val, 1, "range"))?
                    .into_struct_value();
                cur = self
                    .b(self.builder.build_insert_value(cur, i1, 2, "range"))?
                    .into_struct_value();
                Ok(CgValue {
                    value: Some(cur.into()),
                    ty: Type::Range,
                })
            }
            Expr::Array(elems, _) => {
                let elem_ty = if elems.is_empty() {
                    if let Some(Type::Array(inner)) = expected.cloned() {
                        *inner
                    } else {
                        Type::Void
                    }
                } else {
                    self.infer_expr_type(&elems[0])?
                };
                let len_val = self.context.i32_type().const_int(elems.len() as u64, false);
                let elem_size = if elems.is_empty() {
                    self.context.i32_type().const_int(0, false)
                } else {
                    self.elem_size_value(&elem_ty)?
                };
                let f = self
                    .module
                    .get_function("pebbles_array_new")
                    .ok_or_else(|| "missing pebbles_array_new".to_string())?;
                let call = self.b(self.builder.build_call(
                    f,
                    &[
                        BasicMetadataValueEnum::from(elem_size),
                        BasicMetadataValueEnum::from(len_val),
                    ],
                    "arr_new",
                ))?;
                let arr_val = self
                    .call_value(call)
                    .ok_or_else(|| "array_new returned void".to_string())?;
                if !elems.is_empty() {
                    for (idx, expr) in elems.iter().enumerate() {
                        let v = self.codegen_expr(expr, Some(&elem_ty))?;
                        let val = v.value.ok_or_else(|| "array elem expects value".to_string())?;
                        let idx_val = self.context.i32_type().const_int(idx as u64, false);
                        let ptr = self.array_elem_ptr_from_value(arr_val, idx_val, &elem_ty)?;
                        self.b(self.builder.build_store(ptr, val))?;
                    }
                }
                Ok(CgValue {
                    value: Some(arr_val),
                    ty: Type::Array(Box::new(elem_ty)),
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
                        .b(self.builder.build_insert_value(cur, v, idx as u32, "field"))?
                        .into_struct_value();
                }
                Ok(CgValue {
                    value: Some(cur.into()),
                    ty: Type::Named(name.clone()),
                })
            }
            Expr::FieldAccess { obj, field, .. } => {
                let obj_ty = self.infer_expr_type(obj)?;
                if let Type::Array(_) = obj_ty {
                    if field == "length" {
                        let arr = self.codegen_expr(obj, Some(&obj_ty))?;
                        let val = arr.value.ok_or_else(|| "array expects value".to_string())?;
                        let len = self.array_len_from_value(val)?;
                        return Ok(CgValue {
                            value: Some(len.into()),
                            ty: Type::I32,
                        });
                    }
                    return Err("unknown field on array".into());
                }
                let (ptr, field_ty) = self.codegen_field_ptr(obj, field)?;
                let llvm_ty = self.llvm_type(&field_ty);
                let val = self.b(self.builder.build_load(llvm_ty, ptr, field))?;
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
                let res = self.codegen_binop(op.clone(), l, r, &lt)?;
                Ok(CgValue {
                    value: Some(res),
                    ty: self.infer_binop_type(op.clone(), &lt),
                })
            }
            Expr::UnaryOp { op, operand, .. } => {
                let ot = self.infer_expr_type(operand)?;
                let ov = self.codegen_expr(operand, Some(&ot))?;
                let v = ov.value.ok_or_else(|| "unary expects value".to_string())?;
                let res = match op {
                    UnaryOp::Neg => {
                        if ot == Type::F64 {
                            self.b(self.builder.build_float_neg(v.into_float_value(), "fneg"))?
                                .into()
                        } else {
                            self.b(self.builder.build_int_neg(v.into_int_value(), "ineg"))?
                                .into()
                        }
                    }
                    UnaryOp::Not => self
                        .b(self.builder.build_not(v.into_int_value(), "not"))?
                        .into(),
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
                    self.b(self.builder.build_call(
                        f,
                        &[BasicMetadataValueEnum::from(val)],
                        "print",
                    ))?;
                    return Ok(CgValue { value: None, ty: Type::Void });
                }
                if name == "input" {
                    let f = self
                        .module
                        .get_function("pebbles_input")
                        .ok_or_else(|| "missing pebbles_input".to_string())?;
                    let call = self.b(self.builder.build_call(f, &[], "input"))?;
                    let v = self
                        .call_value(call)
                        .ok_or_else(|| "input returned void".to_string())?;
                    return Ok(CgValue { value: Some(v), ty: Type::Str });
                }
                if name == "len" {
                    let arg = args.first().ok_or_else(|| "len expects 1 arg".to_string())?;
                    let arg_ty = self.infer_expr_type(arg)?;
                    if matches!(arg_ty, Type::Array(_)) {
                        let v = self.codegen_expr(arg, Some(&arg_ty))?;
                        let val = v.value.ok_or_else(|| "len expects value".to_string())?;
                        let len = self.array_len_from_value(val)?;
                        return Ok(CgValue { value: Some(len.into()), ty: Type::I32 });
                    }
                    let v = self.codegen_expr(arg, Some(&Type::Str))?;
                    let val = v.value.ok_or_else(|| "len expects value".to_string())?;
                    let f = self
                        .module
                        .get_function("pebbles_len_str")
                        .ok_or_else(|| "missing pebbles_len_str".to_string())?;
                    let call = self.b(self.builder.build_call(
                        f,
                        &[BasicMetadataValueEnum::from(val)],
                        "len",
                    ))?;
                    let v = self
                        .call_value(call)
                        .ok_or_else(|| "len returned void".to_string())?;
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
                        let call = self.b(self.builder.build_call(
                            f,
                            &[BasicMetadataValueEnum::from(val)],
                            "str",
                        ))?;
                        let v = self
                            .call_value(call)
                            .ok_or_else(|| "str returned void".to_string())?;
                        return Ok(CgValue { value: Some(v), ty: Type::Str });
                    }
                    return Err("str() only supports i32 and str for now".into());
                }
                if name == "int" {
                    let arg = args.first().ok_or_else(|| "int expects 1 arg".to_string())?;
                    let v = self.codegen_expr(arg, Some(&Type::Str))?;
                    let val = v.value.ok_or_else(|| "int expects value".to_string())?;
                    let f = self
                        .module
                        .get_function("pebbles_int_str")
                        .ok_or_else(|| "missing pebbles_int_str".to_string())?;
                    let call = self.b(self.builder.build_call(
                        f,
                        &[BasicMetadataValueEnum::from(val)],
                        "int",
                    ))?;
                    let v = self
                        .call_value(call)
                        .ok_or_else(|| "int returned void".to_string())?;
                    return Ok(CgValue { value: Some(v), ty: Type::I32 });
                }
                if name == "float" {
                    let arg = args.first().ok_or_else(|| "float expects 1 arg".to_string())?;
                    let v = self.codegen_expr(arg, Some(&Type::Str))?;
                    let val = v.value.ok_or_else(|| "float expects value".to_string())?;
                    let f = self
                        .module
                        .get_function("pebbles_float_str")
                        .ok_or_else(|| "missing pebbles_float_str".to_string())?;
                    let call = self.b(self.builder.build_call(
                        f,
                        &[BasicMetadataValueEnum::from(val)],
                        "float",
                    ))?;
                    let v = self
                        .call_value(call)
                        .ok_or_else(|| "float returned void".to_string())?;
                    return Ok(CgValue { value: Some(v), ty: Type::F64 });
                }
                if name == "sqrt" {
                    let arg = args.first().ok_or_else(|| "sqrt expects 1 arg".to_string())?;
                    let v = self.codegen_expr(arg, Some(&Type::F64))?;
                    let val = v.value.ok_or_else(|| "sqrt expects value".to_string())?;
                    let f = self
                        .module
                        .get_function("pebbles_sqrt_f64")
                        .ok_or_else(|| "missing pebbles_sqrt_f64".to_string())?;
                    let call = self.b(self.builder.build_call(
                        f,
                        &[BasicMetadataValueEnum::from(val)],
                        "sqrt",
                    ))?;
                    let v = self
                        .call_value(call)
                        .ok_or_else(|| "sqrt returned void".to_string())?;
                    return Ok(CgValue { value: Some(v), ty: Type::F64 });
                }
                let sig = self
                    .sigs
                    .get(name)
                    .ok_or_else(|| format!("unknown function '{name}'"))?
                    .clone();
                let mut llvm_args: Vec<BasicMetadataValueEnum<'ctx>> = Vec::new();
                for (arg, param_ty) in args.iter().zip(sig.params.iter()) {
                    let cv = self.codegen_expr(arg, Some(param_ty))?;
                    let v = cv.value.ok_or_else(|| "call expects value".to_string())?;
                    llvm_args.push(v.into());
                }
                let f = self
                    .functions
                    .get(name)
                    .copied()
                    .ok_or_else(|| format!("missing function '{name}'"))?;
                let call = self.b(self.builder.build_call(f, &llvm_args, "call"))?;
                let ret = if sig.ret == Type::Void {
                    None
                } else {
                    Some(
                        self.call_value(call)
                            .ok_or_else(|| "call returned void".to_string())?,
                    )
                };
                Ok(CgValue { value: ret, ty: sig.ret })
            }
            Expr::MethodCall { obj, method, args, .. } => {
                let obj_ty = self.infer_expr_type(obj)?;
                if let Type::Array(inner) = obj_ty {
                    match method.as_str() {
                        "length" => {
                            let arr = self.codegen_expr(obj, Some(&Type::Array(inner.clone())))?;
                            let val = arr.value.ok_or_else(|| "array expects value".to_string())?;
                            let len = self.array_len_from_value(val)?;
                            return Ok(CgValue { value: Some(len.into()), ty: Type::I32 });
                        }
                        "push" => {
                            let arg = args.first().ok_or_else(|| "push expects 1 arg".to_string())?;
                            let arr_ptr = match obj {
                                Expr::Ident(name, _) => self.lookup_var(name)?,
                                _ => return Err("push requires a mutable array variable".into()),
                            };
                            let v = self.codegen_expr(arg, Some(&inner))?;
                            let val = v.value.ok_or_else(|| "push expects value".to_string())?;
                            let tmp = self.create_entry_alloca("push_tmp", &inner)?;
                            self.b(self.builder.build_store(tmp, val))?;
                            let tmp_cast = self
                                .b(self.builder.build_bit_cast(
                                    tmp,
                                    self.context.i8_type().ptr_type(AddressSpace::default()),
                                    "push_cast",
                                ))?
                                .into_pointer_value();
                            let f = self
                                .module
                                .get_function("pebbles_array_push")
                                .ok_or_else(|| "missing pebbles_array_push".to_string())?;
                            self.b(self.builder.build_call(
                                f,
                                &[
                                    BasicMetadataValueEnum::from(arr_ptr),
                                    BasicMetadataValueEnum::from(tmp_cast),
                                ],
                                "push",
                            ))?;
                            return Ok(CgValue { value: None, ty: Type::Void });
                        }
                        "pop" => {
                            let arr_ptr = match obj {
                                Expr::Ident(name, _) => self.lookup_var(name)?,
                                _ => return Err("pop requires a mutable array variable".into()),
                            };
                            let tmp = self.create_entry_alloca("pop_tmp", &inner)?;
                            let tmp_cast = self
                                .b(self.builder.build_bit_cast(
                                    tmp,
                                    self.context.i8_type().ptr_type(AddressSpace::default()),
                                    "pop_cast",
                                ))?
                                .into_pointer_value();
                            let f = self
                                .module
                                .get_function("pebbles_array_pop")
                                .ok_or_else(|| "missing pebbles_array_pop".to_string())?;
                            let call = self.b(self.builder.build_call(
                                f,
                                &[
                                    BasicMetadataValueEnum::from(arr_ptr),
                                    BasicMetadataValueEnum::from(tmp_cast),
                                ],
                                "pop",
                            ))?;
                            let popped = self
                                .call_value(call)
                                .ok_or_else(|| "pop returned void".to_string())?
                                .into_int_value();
                            let llvm_ty = self
                                .llvm_type(&Type::Optional(inner.clone()))
                                .into_struct_type();
                            let mut val = llvm_ty.get_undef();
                            val = self
                                .b(self.builder.build_insert_value(val, popped, 0, "is_some"))?
                                .into_struct_value();
                            let loaded = self.b(self.builder.build_load(self.llvm_type(&inner), tmp, "pval"))?;
                            val = self
                                .b(self.builder.build_insert_value(val, loaded, 1, "oval"))?
                                .into_struct_value();
                            return Ok(CgValue {
                                value: Some(val.into()),
                                ty: Type::Optional(inner),
                            });
                        }
                        "contains" => {
                            let arg = args.first().ok_or_else(|| "contains expects 1 arg".to_string())?;
                            let arr = self.codegen_expr(obj, Some(&Type::Array(inner.clone())))?;
                            let arr_val = arr.value.ok_or_else(|| "array expects value".to_string())?;
                            let needle = self.codegen_expr(arg, Some(&inner))?;
                            let needle_val = needle.value.ok_or_else(|| "contains expects value".to_string())?;

                            let func = self
                                .current_fn
                                .ok_or_else(|| "contains outside function".to_string())?;
                            let idx_alloca = self.create_entry_alloca("idx", &Type::I32)?;
                            let res_alloca = self.create_entry_alloca("found", &Type::Bool)?;
                            self.b(self.builder.build_store(
                                idx_alloca,
                                self.context.i32_type().const_int(0, false),
                            ))?;
                            self.b(self.builder.build_store(
                                res_alloca,
                                self.context.bool_type().const_int(0, false),
                            ))?;

                            let cond_bb = self.context.append_basic_block(func, "contains.cond");
                            let body_bb = self.context.append_basic_block(func, "contains.body");
                            let end_bb = self.context.append_basic_block(func, "contains.end");

                            self.b(self.builder.build_unconditional_branch(cond_bb))?;
                            self.builder.position_at_end(cond_bb);
                            let cur = self
                                .b(self.builder.build_load(self.context.i32_type(), idx_alloca, "i"))?
                                .into_int_value();
                            let len = self.array_len_from_value(arr_val)?;
                            let cmp = self.b(self.builder.build_int_compare(
                                inkwell::IntPredicate::SLT,
                                cur,
                                len,
                                "cmp",
                            ))?;
                            self.b(self.builder.build_conditional_branch(cmp, body_bb, end_bb))?;

                            self.builder.position_at_end(body_bb);
                            let elem_ptr = self.array_elem_ptr_from_value(arr_val, cur, &inner)?;
                            let elem_val = self.b(self.builder.build_load(self.llvm_type(&inner), elem_ptr, "elem"))?;
                            let eq = self.compare_values(&inner, elem_val, needle_val)?;
                            let prev = self
                                .b(self.builder.build_load(self.context.bool_type(), res_alloca, "prev"))?
                                .into_int_value();
                            let new_val = self.b(self.builder.build_or(prev, eq, "or"))?;
                            self.b(self.builder.build_store(res_alloca, new_val))?;
                            let next = self.b(self.builder.build_int_add(
                                cur,
                                self.context.i32_type().const_int(1, false),
                                "next",
                            ))?;
                            self.b(self.builder.build_store(idx_alloca, next))?;
                            self.b(self.builder.build_unconditional_branch(cond_bb))?;

                            self.builder.position_at_end(end_bb);
                            let found = self
                                .b(self.builder.build_load(self.context.bool_type(), res_alloca, "found"))?;
                            return Ok(CgValue { value: Some(found.into()), ty: Type::Bool });
                        }
                        _ => return Err("unknown array method".into()),
                    }
                }
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
                let mut llvm_args: Vec<BasicMetadataValueEnum<'ctx>> = Vec::new();
                let obj_val = self.codegen_expr(obj, None)?;
                let obj_arg = obj_val.value.ok_or_else(|| "method expects value".to_string())?;
                llvm_args.push(obj_arg.into());
                for arg in args {
                    let cv = self.codegen_expr(arg, None)?;
                    let v = cv.value.ok_or_else(|| "method arg expects value".to_string())?;
                    llvm_args.push(v.into());
                }
                let call = self.b(self.builder.build_call(f, &llvm_args, "mcall"))?;
                let sig = self
                    .sigs
                    .get(&f_name)
                    .ok_or_else(|| format!("missing method sig '{f_name}'"))?
                    .clone();
                let ret = if sig.ret == Type::Void {
                    None
                } else {
                    Some(
                        self.call_value(call)
                            .ok_or_else(|| "method returned void".to_string())?,
                    )
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
            Expr::Index { obj, index, .. } => {
                let obj_ty = self.infer_expr_type(obj)?;
                match obj_ty {
                    Type::Array(inner) => {
                        let arr = self.codegen_expr(obj, Some(&Type::Array(inner.clone())))?;
                        let arr_val = arr.value.ok_or_else(|| "index expects value".to_string())?;
                        let idx = self.codegen_expr(index, Some(&Type::I32))?;
                        let idx_val = idx.value.ok_or_else(|| "index expects value".to_string())?;
                        let ptr = self.array_elem_ptr_from_value(arr_val, idx_val.into_int_value(), &inner)?;
                        let val = self.b(self.builder.build_load(self.llvm_type(&inner), ptr, "idx"))?;
                        Ok(CgValue { value: Some(val.into()), ty: inner })
                    }
                    Type::Str => {
                        let s = self.codegen_expr(obj, Some(&Type::Str))?;
                        let sval = s.value.ok_or_else(|| "index expects value".to_string())?;
                        let idx = self.codegen_expr(index, Some(&Type::I32))?;
                        let idx_val = idx.value.ok_or_else(|| "index expects value".to_string())?;
                        let f = self
                            .module
                            .get_function("pebbles_str_index")
                            .ok_or_else(|| "missing pebbles_str_index".to_string())?;
                        let call = self.b(self.builder.build_call(
                            f,
                            &[
                                BasicMetadataValueEnum::from(sval),
                                BasicMetadataValueEnum::from(idx_val),
                            ],
                            "str_idx",
                        ))?;
                        let v = self
                            .call_value(call)
                            .ok_or_else(|| "str_index returned void".to_string())?;
                        Ok(CgValue { value: Some(v), ty: Type::Str })
                    }
                    _ => Err("indexing not supported".into()),
                }
            }
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

        self.b(
            self.builder
                .build_conditional_branch(cond_v, then_bb, else_bb),
        )?;

        self.builder.position_at_end(then_bb);
        let then_val = self.codegen_block(then, expected)?;
        if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
            self.b(self.builder.build_unconditional_branch(merge_bb))?;
        }
        let then_end = self.builder.get_insert_block().unwrap();

        self.builder.position_at_end(else_bb);
        let else_val = match else_ {
            Some(stmts) => self.codegen_block(stmts, expected)?,
            None => None,
        };
        if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
            self.b(self.builder.build_unconditional_branch(merge_bb))?;
        }
        let else_end = self.builder.get_insert_block().unwrap();

        self.builder.position_at_end(merge_bb);

        let ty = expected.cloned().unwrap_or(Type::Void);
        if ty == Type::Void {
            return Ok(CgValue { value: None, ty });
        }
        let llvm_ty = self.llvm_type(&ty);
        let phi = self.b(self.builder.build_phi(llvm_ty, "iftmp"))?;
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

        self.b(self.builder.build_unconditional_branch(check_bb))?;
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
                self.b(
                    self.builder
                        .build_conditional_branch(cond, arm_bb, end_bb),
                )?;
            } else {
                self.b(
                    self.builder
                        .build_conditional_branch(cond, arm_bb, next_bb),
                )?;
            }

            self.builder.position_at_end(arm_bb);
            self.push_scope();
            for (name, val, ty) in bindings {
                let alloca = self.create_entry_alloca(&name, &ty)?;
                self.b(self.builder.build_store(alloca, val))?;
                self.bind_var(&name, ty, alloca);
            }

            let arm_val = self.codegen_block(&arm.body, expected)?;
            self.pop_scope();

            if exp_ty != Type::Void {
                let v = arm_val.ok_or_else(|| "match arm missing value".to_string())?;
                incoming.push((v, self.builder.get_insert_block().unwrap()));
            }

            if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
                self.b(self.builder.build_unconditional_branch(end_bb))?;
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
        let phi = self.b(self.builder.build_phi(llvm_ty, "matchtmp"))?;
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
                Ok((self.b(self.builder.build_int_compare(inkwell::IntPredicate::EQ, v, c, "m_eq"))?, vec![]))
            }
            Pattern::Bool(b) => {
                let v = value.into_int_value();
                let c = self.context.bool_type().const_int(u64::from(*b), false);
                Ok((self.b(self.builder.build_int_compare(inkwell::IntPredicate::EQ, v, c, "m_eq"))?, vec![]))
            }
            Pattern::Float(f) => {
                let v = value.into_float_value();
                let c = self.context.f64_type().const_float(*f);
                Ok((
                    self.b(self.builder.build_float_compare(
                        inkwell::FloatPredicate::OEQ,
                        v,
                        c,
                        "m_eq",
                    ))?,
                    vec![],
                ))
            }
            Pattern::Str(s) => {
                let f = self
                    .module
                    .get_function("pebbles_str_eq")
                    .ok_or_else(|| "missing pebbles_str_eq".to_string())?;
                let lit: inkwell::values::PointerValue<'ctx> = self
                    .b(self.builder.build_global_string_ptr(s, "mstr"))?
                    .as_pointer_value();
                let call = self.b(self.builder.build_call(
                    f,
                    &[
                        BasicMetadataValueEnum::from(value),
                        BasicMetadataValueEnum::from(lit),
                    ],
                    "streq",
                ))?;
                let v = self
                    .call_value(call)
                    .ok_or_else(|| "streq returned void".to_string())?
                    .into_int_value();
                Ok((v, vec![]))
            }
            Pattern::None => {
                if let Type::Optional(_) = ty {
                    let opt = value.into_struct_value();
                    let is_some = self
                        .b(self.builder.build_extract_value(opt, 0, "is_some"))?
                        .into_int_value();
                    let zero = self.context.bool_type().const_int(0, false);
                    Ok((self.b(self.builder.build_int_compare(inkwell::IntPredicate::EQ, is_some, zero, "isnone"))?, vec![]))
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
                        .b(self.builder.build_extract_value(tup, idx as u32, "te"))?;
                    let t = elem_types.remove(0);
                    let (c, mut b) = self.codegen_pattern_cond(pat, elem, &t)?;
                    cond = self.b(self.builder.build_and(cond, c, "m_and"))?;
                    binds.append(&mut b);
                }
                Ok((cond, binds))
            }
            Pattern::Struct { name, fields } => {
                let st = value.into_struct_value();
                let struct_fields = self
                    .struct_info
                    .get(name)
                    .ok_or_else(|| format!("unknown struct '{name}'"))?
                    .fields
                    .clone();
                let mut field_map: HashMap<String, (usize, Type)> = HashMap::new();
                for (idx, (fname, fty)) in struct_fields.iter().enumerate() {
                    field_map.insert(fname.clone(), (idx, fty.clone()));
                }
                let mut cond = true_val;
                let mut binds = Vec::new();
                for (field_name, pat) in fields {
                    let (idx, fty) = field_map
                        .get(field_name.as_str())
                        .cloned()
                        .ok_or_else(|| format!("unknown field '{field_name}'"))?;
                    let field_val = self
                        .b(self.builder.build_extract_value(st, idx as u32, "sf"))?;
                    let (c, mut b) = self.codegen_pattern_cond(pat, field_val, &fty)?;
                    cond = self.b(self.builder.build_and(cond, c, "m_and"))?;
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

        self.b(self.builder.build_unconditional_branch(cond_bb))?;
        self.builder.position_at_end(cond_bb);
        let cond_val = self.codegen_expr(cond, Some(&Type::Bool))?;
        let cond_v = cond_val
            .value
            .ok_or_else(|| "while condition expects value".to_string())?
            .into_int_value();
        self.b(
            self.builder
                .build_conditional_branch(cond_v, body_bb, end_bb),
        )?;

        self.builder.position_at_end(body_bb);
        self.break_stack.push(end_bb);
        self.continue_stack.push(cond_bb);
        self.codegen_block(body, None)?;
        self.break_stack.pop();
        self.continue_stack.pop();
        if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
            self.b(self.builder.build_unconditional_branch(cond_bb))?;
        }

        self.builder.position_at_end(end_bb);
        Ok(())
    }

    fn codegen_for(&mut self, var: &str, iter: &Expr, body: &[Stmt]) -> Result<(), String> {
        let iter_ty = self.infer_expr_type(iter)?;
        if iter_ty != Type::Range && !matches!(iter_ty, Type::Array(_)) {
            return Err("for loop only supports range or array".into());
        }
        let func = self
            .current_fn
            .ok_or_else(|| "for outside function".to_string())?;

        if iter_ty == Type::Range {
            let range_val = self.codegen_expr(iter, Some(&Type::Range))?;
            let range = range_val
                .value
                .ok_or_else(|| "for range expects value".to_string())?;
            let start = self
                .b(self.builder.build_extract_value(
                    range.into_struct_value(),
                    0,
                    "start",
                ))?
                .into_int_value();
            let end = self
                .b(self.builder.build_extract_value(
                    range.into_struct_value(),
                    1,
                    "end",
                ))?
                .into_int_value();
            let inclusive = self
                .b(self.builder.build_extract_value(
                    range.into_struct_value(),
                    2,
                    "incl",
                ))?
                .into_int_value();

            let idx_alloca = self.create_entry_alloca(var, &Type::I32)?;
            self.b(self.builder.build_store(idx_alloca, start))?;
            self.push_scope();
            self.bind_var(var, Type::I32, idx_alloca);

            let cond_bb = self.context.append_basic_block(func, "for.cond");
            let body_bb = self.context.append_basic_block(func, "for.body");
            let incr_bb = self.context.append_basic_block(func, "for.incr");
            let end_bb = self.context.append_basic_block(func, "for.end");

            self.b(self.builder.build_unconditional_branch(cond_bb))?;
            self.builder.position_at_end(cond_bb);
            let cur = self
                .b(self.builder.build_load(self.context.i32_type(), idx_alloca, "i"))?
                .into_int_value();
            let cmp_le = self.b(self.builder.build_int_compare(
                inkwell::IntPredicate::SLE,
                cur,
                end,
                "cmple",
            ))?;
            let cmp_lt = self.b(self.builder.build_int_compare(
                inkwell::IntPredicate::SLT,
                cur,
                end,
                "cmplt",
            ))?;
            let cmp = self.b(self.builder.build_select(inclusive, cmp_le, cmp_lt, "cmp"))?;
            let cmp_i1 = cmp.into_int_value();
            self.b(
                self.builder
                    .build_conditional_branch(cmp_i1, body_bb, end_bb),
            )?;

            self.builder.position_at_end(body_bb);
            self.break_stack.push(end_bb);
            self.continue_stack.push(incr_bb);
            self.codegen_block(body, None)?;
            self.break_stack.pop();
            self.continue_stack.pop();
            if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
                self.b(self.builder.build_unconditional_branch(incr_bb))?;
            }

            self.builder.position_at_end(incr_bb);
            let cur = self
                .b(self.builder.build_load(self.context.i32_type(), idx_alloca, "i"))?
                .into_int_value();
            let next = self.b(self.builder.build_int_add(
                cur,
                self.context.i32_type().const_int(1, false),
                "i.next",
            ))?;
            self.b(self.builder.build_store(idx_alloca, next))?;
            self.b(self.builder.build_unconditional_branch(cond_bb))?;

            self.builder.position_at_end(end_bb);
            self.pop_scope();
            return Ok(());
        }

        let elem_ty = match iter_ty {
            Type::Array(inner) => *inner,
            _ => return Err("for loop only supports range or array".into()),
        };
        let arr_val = self.codegen_expr(iter, Some(&Type::Array(Box::new(elem_ty.clone()))))?;
        let arr = arr_val
            .value
            .ok_or_else(|| "for array expects value".to_string())?;
        let arr_alloca = self.create_entry_alloca("arr", &Type::Array(Box::new(elem_ty.clone())))?;
        self.b(self.builder.build_store(arr_alloca, arr))?;

        let idx_alloca = self.create_entry_alloca("i", &Type::I32)?;
        self.b(self.builder.build_store(
            idx_alloca,
            self.context.i32_type().const_int(0, false),
        ))?;
        self.push_scope();
        let var_alloca = self.create_entry_alloca(var, &elem_ty)?;
        self.bind_var(var, elem_ty.clone(), var_alloca);

        let cond_bb = self.context.append_basic_block(func, "for.cond");
        let body_bb = self.context.append_basic_block(func, "for.body");
        let incr_bb = self.context.append_basic_block(func, "for.incr");
        let end_bb = self.context.append_basic_block(func, "for.end");

        self.b(self.builder.build_unconditional_branch(cond_bb))?;
        self.builder.position_at_end(cond_bb);
        let cur = self
            .b(self.builder.build_load(self.context.i32_type(), idx_alloca, "i"))?
            .into_int_value();
        let arr_loaded = self.b(self.builder.build_load(
            self.llvm_type(&Type::Array(Box::new(elem_ty.clone()))),
            arr_alloca,
            "arr",
        ))?;
        let len = self.array_len_from_value(arr_loaded.into())?;
        let cmp = self.b(self.builder.build_int_compare(
            inkwell::IntPredicate::SLT,
            cur,
            len,
            "cmplt",
        ))?;
        self.b(self.builder.build_conditional_branch(cmp, body_bb, end_bb))?;

        self.builder.position_at_end(body_bb);
        let elem_ptr = self.array_elem_ptr_from_value(arr_loaded.into(), cur, &elem_ty)?;
        let elem_val = self.b(self.builder.build_load(self.llvm_type(&elem_ty), elem_ptr, "elem"))?;
        self.b(self.builder.build_store(var_alloca, elem_val))?;
        self.break_stack.push(end_bb);
        self.continue_stack.push(incr_bb);
        self.codegen_block(body, None)?;
        self.break_stack.pop();
        self.continue_stack.pop();
        if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
            self.b(self.builder.build_unconditional_branch(incr_bb))?;
        }

        self.builder.position_at_end(incr_bb);
        let next = self.b(self.builder.build_int_add(
            cur,
            self.context.i32_type().const_int(1, false),
            "next",
        ))?;
        self.b(self.builder.build_store(idx_alloca, next))?;
        self.b(self.builder.build_unconditional_branch(cond_bb))?;

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
                        BinOp::Add => self.b(self.builder.build_float_add(lv, rv, "fadd"))?,
                        BinOp::Sub => self.b(self.builder.build_float_sub(lv, rv, "fsub"))?,
                        BinOp::Mul => self.b(self.builder.build_float_mul(lv, rv, "fmul"))?,
                        BinOp::Div => self.b(self.builder.build_float_div(lv, rv, "fdiv"))?,
                        BinOp::Mod => self.b(self.builder.build_float_rem(lv, rv, "frem"))?,
                        _ => unreachable!(),
                    };
                    Ok(v.into())
                } else if *ty == Type::Str && op == BinOp::Add {
                    let f = self
                        .module
                        .get_function("pebbles_str_concat")
                        .ok_or_else(|| "missing pebbles_str_concat".to_string())?;
                    let call = self.b(self.builder.build_call(
                        f,
                        &[
                            BasicMetadataValueEnum::from(l),
                            BasicMetadataValueEnum::from(r),
                        ],
                        "concat",
                    ))?;
                    Ok(self
                        .call_value(call)
                        .ok_or_else(|| "concat returned void".to_string())?)
                } else {
                    let lv = l.into_int_value();
                    let rv = r.into_int_value();
                    let v = match op {
                        BinOp::Add => self.b(self.builder.build_int_add(lv, rv, "iadd"))?,
                        BinOp::Sub => self.b(self.builder.build_int_sub(lv, rv, "isub"))?,
                        BinOp::Mul => self.b(self.builder.build_int_mul(lv, rv, "imul"))?,
                        BinOp::Div => self
                            .b(self.builder.build_int_signed_div(lv, rv, "idiv"))?,
                        BinOp::Mod => self
                            .b(self.builder.build_int_signed_rem(lv, rv, "irem"))?,
                        _ => unreachable!(),
                    };
                    Ok(v.into())
                }
            }
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
                if matches!(ty, Type::Tuple(_) | Type::Optional(_) | Type::Named(_) | Type::Range | Type::Array(_)) {
                    let mut v = self.compare_values(ty, l, r)?;
                    if op == BinOp::NotEq {
                        v = self.b(self.builder.build_not(v, "not"))?;
                    }
                    return Ok(v.into());
                }
                if *ty == Type::Str && (op == BinOp::Eq || op == BinOp::NotEq) {
                    let f = self
                        .module
                        .get_function("pebbles_str_eq")
                        .ok_or_else(|| "missing pebbles_str_eq".to_string())?;
                    let call = self.b(self.builder.build_call(
                        f,
                        &[
                            BasicMetadataValueEnum::from(l),
                            BasicMetadataValueEnum::from(r),
                        ],
                        "streq",
                    ))?;
                    let mut v = self
                        .call_value(call)
                        .ok_or_else(|| "streq returned void".to_string())?
                        .into_int_value();
                    if op == BinOp::NotEq {
                        v = self.b(self.builder.build_not(v, "not"))?;
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
                    Ok(self
                        .b(self.builder.build_float_compare(pred, lv, rv, "fcmp"))?
                        .into())
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
                    Ok(self
                        .b(self.builder.build_int_compare(pred, lv, rv, "icmp"))?
                        .into())
                }
            }
            BinOp::And | BinOp::Or => {
                let lv = l.into_int_value();
                let rv = r.into_int_value();
                let v = match op {
                    BinOp::And => self.b(self.builder.build_and(lv, rv, "and"))?,
                    BinOp::Or => self.b(self.builder.build_or(lv, rv, "or"))?,
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
                .b(self.builder.build_signed_int_to_float(
                    val.into_int_value(),
                    self.context.f64_type(),
                    "sitofp",
                ))?
                .into()),
            (Type::F64, Type::I32) => Ok(self
                .b(self.builder.build_float_to_signed_int(
                    val.into_float_value(),
                    self.context.i32_type(),
                    "fptosi",
                ))?
                .into()),
            (Type::F64, Type::I64) => Ok(self
                .b(self.builder.build_float_to_signed_int(
                    val.into_float_value(),
                    self.context.i64_type(),
                    "fptosi",
                ))?
                .into()),
            (Type::I32, Type::I64) => Ok(self
                .b(self.builder.build_int_cast(
                    val.into_int_value(),
                    self.context.i64_type(),
                    "sext",
                ))?
                .into()),
            (Type::I64, Type::I32) => Ok(self
                .b(self.builder.build_int_cast(
                    val.into_int_value(),
                    self.context.i32_type(),
                    "trunc",
                ))?
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
            AssignTarget::Index { obj, index, .. } => {
                let obj_ty = self.infer_expr_type(obj)?;
                let elem_ty = match obj_ty {
                    Type::Array(inner) => *inner,
                    _ => return Err("index assignment only supports arrays".into()),
                };
                let arr_ptr = match obj {
                    Expr::Ident(name, _) => self.lookup_var(name)?,
                    _ => return Err("index assignment requires array variable".into()),
                };
                let arr_val = self.b(self.builder.build_load(
                    self.llvm_type(&Type::Array(Box::new(elem_ty.clone()))),
                    arr_ptr,
                    "arr",
                ))?;
                let idx = self.codegen_expr(index, Some(&Type::I32))?;
                let idx_val = idx.value.ok_or_else(|| "index expects value".to_string())?;
                let elem_ptr = self.array_elem_ptr_from_value(arr_val, idx_val.into_int_value(), &elem_ty)?;
                Ok((elem_ptr, elem_ty))
            }
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
        let st = self
            .structs
            .get(&struct_name)
            .copied()
            .ok_or_else(|| format!("missing struct '{struct_name}'"))?;
        let (field_idx, field_ty) = info
            .fields
            .iter()
            .enumerate()
            .find(|(_, (n, _))| n == field)
            .map(|(i, (_, t))| (i, t.clone()))
            .ok_or_else(|| format!("unknown field '{field}'"))?;

        let base_ptr = self.codegen_lvalue(obj)?;
        let field_ptr = unsafe {
            self.b(self.builder.build_struct_gep(
                st,
                base_ptr,
                field_idx as u32,
                "fieldptr",
            ))?
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
                self.b(self.builder.build_store(alloca, v))?;
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
        Ok(self.b(builder.build_alloca(llvm_ty, name))?)
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
                if let Type::Array(_) = obj_ty {
                    if field == "length" {
                        return Ok(Type::I32);
                    }
                    return Err("unknown field on array".into());
                }
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
                Ok(self.infer_binop_type(op.clone(), &lt))
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
                if let Type::Array(inner) = obj_ty {
                    return match method.as_str() {
                        "length" => Ok(Type::I32),
                        "push" => Ok(Type::Void),
                        "pop" => Ok(Type::Optional(inner)),
                        "contains" => Ok(Type::Bool),
                        _ => Err("unknown array method".into()),
                    };
                }
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
            Expr::Array(elems, _) => {
                if elems.is_empty() {
                    Ok(Type::Array(Box::new(Type::Void)))
                } else {
                    let elem = self.infer_expr_type(&elems[0])?;
                    Ok(Type::Array(Box::new(elem)))
                }
            }
            Expr::Index { obj, .. } => {
                let obj_ty = self.infer_expr_type(obj)?;
                match obj_ty {
                    Type::Array(inner) => Ok(*inner),
                    Type::Str => Ok(Type::Str),
                    _ => Err("indexing not supported".into()),
                }
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
                let _ = inner;
                self.array_ty.into()
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
