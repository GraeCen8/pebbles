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

    fn define_items(&mut self, _items: &[Item]) -> Result<(), String> {
        Ok(())
    }
}
