// Walks the AST produced by the parser and:
//   1. Infers types for every expression
//   2. Checks types are compatible at every operation
//   3. Resolves identifiers against the current scope stack
//   4. Validates calls, struct literals, field access, and method calls
//   5. Enforces mutability rules on assignment
//   6. Checks return types match function signatures
//
// Entry point:
//   let mut tc = TypeChecker::new();
//   tc.check(&items)?;  // items: &Vec<Item>
 
use std::collections::HashMap;
use crate::ast::*;
 
// ─── Error ────────────────────────────────────────────────────────────────────
 
#[derive(Debug)]
pub struct TypeError {
    pub message: String,
    pub line: usize,
}
 
impl TypeError {
    fn new(msg: impl Into<String>, line: usize) -> Self {
        Self { message: msg.into(), line }
    }
}
 
impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "type error on line {}: {}", self.line, self.message)
    }
}
 
pub type TypeResult<T> = Result<T, TypeError>;
 
// ─── Scope stack ──────────────────────────────────────────────────────────────
 
/// One lexical scope: maps name → (type, is_mutable)
type Scope = HashMap<String, (Type, bool)>;
 
struct ScopeStack {
    scopes: Vec<Scope>,
}
 
impl ScopeStack {
    fn new() -> Self {
        Self { scopes: vec![HashMap::new()] }
    }
 
    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }
 
    fn pop(&mut self) {
        self.scopes.pop();
    }
 
    fn declare(&mut self, name: String, ty: Type, mutable: bool) {
        self.scopes.last_mut().unwrap().insert(name, (ty, mutable));
    }
 
    fn lookup(&self, name: &str) -> Option<&(Type, bool)> {
        for scope in self.scopes.iter().rev() {
            if let Some(entry) = scope.get(name) {
                return Some(entry);
            }
        }
        None
    }
}
 
// ─── Function signature ───────────────────────────────────────────────────────
 
#[derive(Debug, Clone)]
struct FnSig {
    params: Vec<(String, Type)>,
    ret: Type,
}
 
// ─── Struct definition (resolved) ─────────────────────────────────────────────
 
#[derive(Debug, Clone)]
struct StructInfo {
    fields: Vec<(String, Type)>,
    methods: HashMap<String, FnSig>,
}
 
// ─── Type checker ─────────────────────────────────────────────────────────────
 
pub struct TypeChecker {
    scopes:      ScopeStack,
    functions:   HashMap<String, FnSig>,
    structs:     HashMap<String, StructInfo>,
    /// Return type of the function currently being checked.
    current_ret: Option<Type>,
    /// Whether a return statement has been seen on the current path.
    returned:    bool,
}
 
impl TypeChecker {
    pub fn new() -> Self {
        Self {
            scopes:      ScopeStack::new(),
            functions:   Self::builtin_fns(),
            structs:     HashMap::new(),
            current_ret: None,
            returned:    false,
        }
    }
 
    // ── Built-in functions ────────────────────────────────────────────────────
 
    fn builtin_fns() -> HashMap<String, FnSig> {
        let mut m = HashMap::new();
        m.insert("print".into(),  FnSig { params: vec![("x".into(), Type::Str)],  ret: Type::Void });
        m.insert("input".into(),  FnSig { params: vec![],                          ret: Type::Str  });
        m.insert("len".into(),    FnSig { params: vec![("x".into(), Type::Str)],  ret: Type::I32  });
        m.insert("str".into(),    FnSig { params: vec![("x".into(), Type::I32)],  ret: Type::Str  });
        m.insert("int".into(),    FnSig { params: vec![("x".into(), Type::Str)],  ret: Type::I32  });
        m.insert("float".into(),  FnSig { params: vec![("x".into(), Type::Str)],  ret: Type::F64  });
        m.insert("sqrt".into(),   FnSig { params: vec![("x".into(), Type::F64)],  ret: Type::F64  });
        m
    }
 
    // ─── Entry point ──────────────────────────────────────────────────────────
 
    pub fn check(&mut self, items: &[Item]) -> TypeResult<()> {
        self.collect_signatures(items)?;
        for item in items {
            self.check_item(item)?;
        }
        Ok(())
    }
 
    // ─── Pass 1: collect signatures ───────────────────────────────────────────
 
    fn collect_signatures(&mut self, items: &[Item]) -> TypeResult<()> {
        for item in items {
            match item {
                Item::Fn(f)     => self.register_fn(f)?,
                Item::Struct(s) => self.register_struct(s)?,
                Item::Impl(i)   => self.register_impl(i)?,
            }
        }
        Ok(())
    }
 
    fn register_fn(&mut self, f: &FnDef) -> TypeResult<()> {
        if self.functions.contains_key(&f.name) {
            return Err(TypeError::new(format!("function '{}' already defined", f.name), f.line));
        }
        let sig = FnSig {
            params: f.params.iter()
                .filter(|p| p.name != "self")
                .map(|p| (p.name.clone(), p.ty.clone()))
                .collect(),
            ret: f.ret.clone(),
        };
        self.functions.insert(f.name.clone(), sig);
        Ok(())
    }
 
    fn register_struct(&mut self, s: &StructDef) -> TypeResult<()> {
        if self.structs.contains_key(&s.name) {
            return Err(TypeError::new(format!("struct '{}' already defined", s.name), s.line));
        }
        self.structs.insert(s.name.clone(), StructInfo {
            fields: s.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect(),
            methods: HashMap::new(),
        });
        Ok(())
    }
 
    fn register_impl(&mut self, i: &ImplBlock) -> TypeResult<()> {
        if !self.structs.contains_key(&i.type_name) {
            return Err(TypeError::new(
                format!("impl for unknown struct '{}'", i.type_name), i.line,
            ));
        }
        for method in &i.methods {
            let sig = FnSig {
                params: method.params.iter()
                    .filter(|p| p.name != "self")
                    .map(|p| (p.name.clone(), p.ty.clone()))
                    .collect(),
                ret: method.ret.clone(),
            };
            self.structs.get_mut(&i.type_name).unwrap()
                .methods.insert(method.name.clone(), sig);
        }
        Ok(())
    }
 
    // ─── Pass 2: check items ──────────────────────────────────────────────────
 
    fn check_item(&mut self, item: &Item) -> TypeResult<()> {
        match item {
            Item::Fn(f)     => self.check_fn(f, None),
            Item::Struct(_) => Ok(()),
            Item::Impl(i)   => {
                for method in &i.methods {
                    self.check_fn(method, Some(&i.type_name.clone()))?;
                }
                Ok(())
            }
        }
    }
 
    fn check_fn(&mut self, f: &FnDef, self_type: Option<&str>) -> TypeResult<()> {
        let prev_ret      = self.current_ret.replace(f.ret.clone());
        let prev_returned = std::mem::replace(&mut self.returned, false);
 
        self.scopes.push();
 
        // Bind 'self' for methods
        if let Some(type_name) = self_type {
            let mutable = f.params.first().map_or(false, |p| p.mutable);
            self.scopes.declare("self".into(), Type::Named(type_name.to_string()), mutable);
        }
 
        // Bind parameters into scope
        for param in &f.params {
            if param.name == "self" { continue; }
            self.scopes.declare(param.name.clone(), param.ty.clone(), param.mutable);
        }
 
        self.check_block(&f.body)?;
 
        self.scopes.pop();
        self.current_ret = prev_ret;
        self.returned    = prev_returned;
        Ok(())
    }
 
    // ─── Block ────────────────────────────────────────────────────────────────
 
    fn check_block(&mut self, stmts: &[Stmt]) -> TypeResult<()> {
        self.scopes.push();
        for stmt in stmts {
            self.check_stmt(stmt)?;
        }
        self.scopes.pop();
        Ok(())
    }
 
    // ─── Statements ───────────────────────────────────────────────────────────
 
    fn check_stmt(&mut self, stmt: &Stmt) -> TypeResult<()> {
        match stmt {
            Stmt::Let { name, ty, value, mutable, line } => {
                let val_ty = self.infer_expr(value)?;
                let declared_ty = if let Some(ann) = ty {
                    self.assert_assignable(&val_ty, ann, *line)?;
                    ann.clone()
                } else {
                    val_ty
                };
                self.scopes.declare(name.clone(), declared_ty, *mutable);
                Ok(())
            }
 
            Stmt::Assign { target, value, line } => {
                let (target_ty, mutable) = self.check_assign_target(target)?;
                if !mutable {
                    return Err(TypeError::new("cannot assign to immutable variable", *line));
                }
                let val_ty = self.infer_expr(value)?;
                self.assert_assignable(&val_ty, &target_ty, *line)
            }
 
            Stmt::Return { value, line } => {
                let ret_ty = self.current_ret.clone().ok_or_else(|| {
                    TypeError::new("return outside of function", *line)
                })?;
                let val_ty = match value {
                    Some(e) => self.infer_expr(e)?,
                    None    => Type::Void,
                };
                self.assert_assignable(&val_ty, &ret_ty, *line)?;
                self.returned = true;
                Ok(())
            }
 
            Stmt::While { cond, body, line } => {
                let cond_ty = self.infer_expr(cond)?;
                self.assert_type(&cond_ty, &Type::Bool, *line)?;
                self.check_block(body)
            }
 
            Stmt::For { var, iter, body, line } => {
                let iter_ty = self.infer_expr(iter)?;
                let elem_ty = match &iter_ty {
                    Type::Array(inner) => *inner.clone(),
                    Type::Range        => Type::I32,
                    other => return Err(TypeError::new(
                        format!("cannot iterate over {:?}", other), *line,
                    )),
                };
                self.scopes.push();
                self.scopes.declare(var.clone(), elem_ty, false);
                self.check_block(body)?;
                self.scopes.pop();
                Ok(())
            }
 
            Stmt::Break { .. } | Stmt::Continue { .. } => Ok(()),
 
            Stmt::Expr(expr) => {
                self.infer_expr(expr)?;
                Ok(())
            }
        }
    }
 
    // ─── Assignment target ────────────────────────────────────────────────────
 
    fn check_assign_target(&self, target: &AssignTarget) -> TypeResult<(Type, bool)> {
        match target {
            AssignTarget::Ident(name, line) => {
                self.scopes.lookup(name)
                    .map(|(ty, m)| (ty.clone(), *m))
                    .ok_or_else(|| TypeError::new(format!("undefined variable '{}'", name), *line))
            }
 
            AssignTarget::Field { obj, field, line } => {
                let obj_ty  = self.infer_expr(obj)?;
                let mutable = self.expr_is_mutable(obj);
                let field_ty = self.lookup_field(&obj_ty, field, *line)?;
                Ok((field_ty, mutable))
            }
 
            AssignTarget::Index { obj, index, line } => {
                let obj_ty  = self.infer_expr(obj)?;
                let mutable = self.expr_is_mutable(obj);
                let elem_ty = match &obj_ty {
                    Type::Array(inner) => *inner.clone(),
                    other => return Err(TypeError::new(
                        format!("cannot index into {:?}", other), *line,
                    )),
                };
                let idx_ty = self.infer_expr(index)?;
                self.assert_type(&idx_ty, &Type::I32, *line)?;
                Ok((elem_ty, mutable))
            }
        }
    }
 
    fn expr_is_mutable(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(name, _) => self.scopes.lookup(name).map_or(false, |(_, m)| *m),
            _ => true, // be permissive for complex lvalues
        }
    }

    fn array_obj_mutable(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(name, _) => self.scopes.lookup(name).map_or(false, |(_, m)| *m),
            _ => false,
        }
    }
 
    // ─── Expression inference ─────────────────────────────────────────────────
 
    pub fn infer_expr(&self, expr: &Expr) -> TypeResult<Type> {
        match expr {
            // Literals
            Expr::Int(_, _)   => Ok(Type::I32),
            Expr::Float(_, _) => Ok(Type::F64),
            Expr::Bool(_, _)  => Ok(Type::Bool),
            Expr::Str(_, _)   => Ok(Type::Str),
            Expr::None(_)     => Ok(Type::Optional(Box::new(Type::Void))),
 
            // Collections
            Expr::Array(elems, line) => {
                if elems.is_empty() {
                    return Ok(Type::Array(Box::new(Type::Void)));
                }
                let first = self.infer_expr(&elems[0])?;
                for e in elems.iter().skip(1) {
                    let t = self.infer_expr(e)?;
                    self.assert_same(&t, &first, *line)?;
                }
                Ok(Type::Array(Box::new(first)))
            }
 
            Expr::Tuple(elems, _) => {
                let tys: TypeResult<Vec<Type>> = elems.iter().map(|e| self.infer_expr(e)).collect();
                Ok(Type::Tuple(tys?))
            }
 
            // Range: both bounds must be i32
            Expr::Range { start, end, line, .. } => {
                let s = self.infer_expr(start)?;
                let e = self.infer_expr(end)?;
                self.assert_type(&s, &Type::I32, *line)?;
                self.assert_type(&e, &Type::I32, *line)?;
                Ok(Type::Range)
            }
 
            // Identifier lookup
            Expr::Ident(name, line) => {
                self.scopes.lookup(name)
                    .map(|(ty, _)| ty.clone())
                    .ok_or_else(|| TypeError::new(format!("undefined variable '{}'", name), *line))
            }
 
            // Binary operators
            Expr::BinOp { op, left, right, line } => {
                let lt = self.infer_expr(left)?;
                let rt = self.infer_expr(right)?;
                match op {
                    BinOp::And | BinOp::Or => {
                        self.assert_type(&lt, &Type::Bool, *line)?;
                        self.assert_type(&rt, &Type::Bool, *line)?;
                        Ok(Type::Bool)
                    }
                    BinOp::Eq | BinOp::NotEq => {
                        self.assert_same(&lt, &rt, *line)?;
                        Ok(Type::Bool)
                    }
                    BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
                        self.assert_numeric(&lt, *line)?;
                        self.assert_same(&lt, &rt, *line)?;
                        Ok(Type::Bool)
                    }
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                        // str + str = string concatenation
                        if *op == BinOp::Add && lt == Type::Str && rt == Type::Str {
                            return Ok(Type::Str);
                        }
                        self.assert_numeric(&lt, *line)?;
                        self.assert_same(&lt, &rt, *line)?;
                        Ok(lt)
                    }
                }
            }
 
            // Unary operators
            Expr::UnaryOp { op, operand, line } => {
                let ty = self.infer_expr(operand)?;
                match op {
                    UnaryOp::Neg => { self.assert_numeric(&ty, *line)?; Ok(ty) }
                    UnaryOp::Not => { self.assert_type(&ty, &Type::Bool, *line)?; Ok(Type::Bool) }
                }
            }
 
            // Cast: as
            Expr::Cast { expr, ty, line } => {
                let from = self.infer_expr(expr)?;
                self.assert_castable(&from, ty, *line)?;
                Ok(ty.clone())
            }
 
            // Field access: obj.field
            Expr::FieldAccess { obj, field, line } => {
                let obj_ty = self.infer_expr(obj)?;
                self.lookup_field(&obj_ty, field, *line)
            }
 
            // Index: obj[i]
            Expr::Index { obj, index, line } => {
                let obj_ty = self.infer_expr(obj)?;
                let idx_ty = self.infer_expr(index)?;
                self.assert_type(&idx_ty, &Type::I32, *line)?;
                match obj_ty {
                    Type::Array(inner) => Ok(*inner),
                    Type::Str          => Ok(Type::Str),
                    other => Err(TypeError::new(format!("cannot index into {:?}", other), *line)),
                }
            }
 
            // Function call
            Expr::Call { name, args, line } => {
                self.check_call(name, args, *line)
            }
 
            // Method call: obj.method(args)
            Expr::MethodCall { obj, method, args, line } => {
                let obj_ty = self.infer_expr(obj)?;
                match &obj_ty {
                    Type::Array(_) => {
                        let mutable = self.array_obj_mutable(obj);
                        self.check_array_method(&obj_ty, method, args, *line, mutable)
                    }
                    Type::Named(struct_name) => {
                        let sig = self.structs
                            .get(struct_name)
                            .and_then(|s| s.methods.get(method))
                            .cloned()
                            .ok_or_else(|| TypeError::new(
                                format!("no method '{}' on '{}'", method, struct_name), *line,
                            ))?;
                        self.check_args(&sig.params, args, *line)?;
                        Ok(sig.ret)
                    }
                    other => Err(TypeError::new(
                        format!("cannot call method on {:?}", other), *line,
                    )),
                }
            }
 
            // Struct literal: Name { field: val, ... }
            Expr::StructLit { name, fields, line } => {
                let info = self.structs.get(name).cloned().ok_or_else(|| {
                    TypeError::new(format!("undefined struct '{}'", name), *line)
                })?;
 
                let provided: HashMap<&str, &Expr> =
                    fields.iter().map(|(n, e)| (n.as_str(), e)).collect();
 
                // Every declared field must be provided with the right type
                for (fname, fty) in &info.fields {
                    match provided.get(fname.as_str()) {
                        None => return Err(TypeError::new(
                            format!("missing field '{}' in '{}' literal", fname, name), *line,
                        )),
                        Some(val) => {
                            let val_ty = self.infer_expr(val)?;
                            self.assert_assignable(&val_ty, fty, *line)?;
                        }
                    }
                }
 
                // No unknown fields
                for (fname, _) in fields {
                    if !info.fields.iter().any(|(n, _)| n == fname) {
                        return Err(TypeError::new(
                            format!("unknown field '{}' in struct '{}'", fname, name), *line,
                        ));
                    }
                }
 
                Ok(Type::Named(name.clone()))
            }
 
            // If expression
            Expr::If { cond, then, else_, line } => {
                let cond_ty = self.infer_expr(cond)?;
                self.assert_type(&cond_ty, &Type::Bool, *line)?;
 
                let then_ty = self.block_tail_type(then)?;
 
                match else_ {
                    None => Ok(Type::Void),
                    Some(else_stmts) => {
                        let else_ty = self.block_tail_type(else_stmts)?;
                        match (then_ty, else_ty) {
                            (Some(t), Some(e)) => {
                                self.assert_same(&t, &e, *line)?;
                                Ok(t)
                            }
                            _ => Ok(Type::Void),
                        }
                    }
                }
            }
 
            // Match expression
            Expr::Match { subject, arms, line } => {
                let subject_ty = self.infer_expr(subject)?;
                let mut result_ty: Option<Type> = None;
 
                for arm in arms {
                    self.check_pattern(&arm.pattern, &subject_ty, *line)?;
                    let arm_ty = self.block_tail_type(&arm.body)?;
                    match (&result_ty, arm_ty) {
                        (None, Some(t)) => result_ty = Some(t),
                        (Some(existing), Some(t)) => self.assert_same(existing, &t, *line)?,
                        _ => {}
                    }
                }
 
                Ok(result_ty.unwrap_or(Type::Void))
            }
        }
    }
 
    // ─── Block tail type (type of last expression in block) ───────────────────
 
    fn block_tail_type(&self, stmts: &[Stmt]) -> TypeResult<Option<Type>> {
        match stmts.last() {
            Some(Stmt::Expr(e)) => Ok(Some(self.infer_expr(e)?)),
            _ => Ok(None),
        }
    }
 
    // ─── Call helpers ─────────────────────────────────────────────────────────
 
    fn check_call(&self, name: &str, args: &[Expr], line: usize) -> TypeResult<Type> {
        // len() accepts any array or str
        if name == "len" {
            if args.len() != 1 {
                return Err(TypeError::new("len() takes exactly 1 argument", line));
            }
            return match self.infer_expr(&args[0])? {
                Type::Array(_) | Type::Str => Ok(Type::I32),
                other => Err(TypeError::new(format!("len() expects array or str, got {:?}", other), line)),
            };
        }
 
        // str() accepts any type
        if name == "str" {
            if args.len() != 1 {
                return Err(TypeError::new("str() takes exactly 1 argument", line));
            }
            self.infer_expr(&args[0])?;
            return Ok(Type::Str);
        }
 
        let sig = self.functions.get(name).cloned().ok_or_else(|| {
            TypeError::new(format!("undefined function '{}'", name), line)
        })?;
        self.check_args(&sig.params, args, line)?;
        Ok(sig.ret)
    }
 
    fn check_args(&self, params: &[(String, Type)], args: &[Expr], line: usize) -> TypeResult<()> {
        if args.len() != params.len() {
            return Err(TypeError::new(
                format!("expected {} argument(s), got {}", params.len(), args.len()), line,
            ));
        }
        for ((_, param_ty), arg) in params.iter().zip(args.iter()) {
            let arg_ty = self.infer_expr(arg)?;
            self.assert_assignable(&arg_ty, param_ty, line)?;
        }
        Ok(())
    }
 
    // ─── Array built-in methods ───────────────────────────────────────────────
 
    fn check_array_method(
        &self,
        arr_ty: &Type,
        method: &str,
        args: &[Expr],
        line: usize,
        mutable: bool,
    ) -> TypeResult<Type> {
        let elem_ty = match arr_ty {
            Type::Array(inner) => *inner.clone(),
            _ => unreachable!(),
        };
        match method {
            "length" if args.is_empty()  => Ok(Type::I32),
            "push"   if args.len() == 1  => {
                if !mutable {
                    return Err(TypeError::new("push() requires a mutable array", line));
                }
                let arg_ty = self.infer_expr(&args[0])?;
                self.assert_assignable(&arg_ty, &elem_ty, line)?;
                Ok(Type::Void)
            }
            "pop"      if args.is_empty() => {
                if !mutable {
                    return Err(TypeError::new("pop() requires a mutable array", line));
                }
                Ok(Type::Optional(Box::new(elem_ty)))
            }
            "contains" if args.len() == 1 => {
                if matches!(elem_ty, Type::Array(_)) {
                    return Err(TypeError::new("contains() does not support array elements yet", line));
                }
                let arg_ty = self.infer_expr(&args[0])?;
                self.assert_same(&arg_ty, &elem_ty, line)?;
                Ok(Type::Bool)
            }
            _ => Err(TypeError::new(format!("no method '{}' on array", method), line)),
        }
    }
 
    // ─── Pattern checking ─────────────────────────────────────────────────────
 
    fn check_pattern(&self, pattern: &Pattern, subject_ty: &Type, line: usize) -> TypeResult<()> {
        match pattern {
            Pattern::Wildcard | Pattern::Binding(_) => Ok(()),
            Pattern::Int(_)   => self.assert_type(subject_ty, &Type::I32,  line),
            Pattern::Float(_) => self.assert_type(subject_ty, &Type::F64,  line),
            Pattern::Bool(_)  => self.assert_type(subject_ty, &Type::Bool, line),
            Pattern::Str(_)   => self.assert_type(subject_ty, &Type::Str,  line),
            Pattern::None     => match subject_ty {
                Type::Optional(_) => Ok(()),
                _ => Err(TypeError::new("none pattern only matches optional types", line)),
            },
            Pattern::Tuple(pats) => match subject_ty {
                Type::Tuple(tys) => {
                    if pats.len() != tys.len() {
                        return Err(TypeError::new(
                            format!("tuple pattern length mismatch: {} vs {}", pats.len(), tys.len()), line,
                        ));
                    }
                    for (p, t) in pats.iter().zip(tys.iter()) {
                        self.check_pattern(p, t, line)?;
                    }
                    Ok(())
                }
                _ => Err(TypeError::new("tuple pattern requires a tuple subject", line)),
            },
            Pattern::Struct { name, fields } => match subject_ty {
                Type::Named(n) if n == name => {
                    let info = self.structs.get(name).ok_or_else(|| {
                        TypeError::new(format!("unknown struct '{}'", name), line)
                    })?;
                    for (fname, fpat) in fields {
                        let field_ty = info.fields.iter()
                            .find(|(n, _)| n == fname)
                            .map(|(_, t)| t)
                            .ok_or_else(|| TypeError::new(
                                format!("unknown field '{}' in struct '{}'", fname, name), line,
                            ))?;
                        self.check_pattern(fpat, field_ty, line)?;
                    }
                    Ok(())
                }
                _ => Err(TypeError::new(
                    format!("struct pattern '{}' does not match subject type", name), line,
                )),
            },
        }
    }
 
    // ─── Field lookup ─────────────────────────────────────────────────────────
 
    fn lookup_field(&self, obj_ty: &Type, field: &str, line: usize) -> TypeResult<Type> {
        match obj_ty {
            Type::Named(struct_name) => {
                let info = self.structs.get(struct_name).ok_or_else(|| {
                    TypeError::new(format!("undefined struct '{}'", struct_name), line)
                })?;
                info.fields.iter()
                    .find(|(name, _)| name == field)
                    .map(|(_, ty)| ty.clone())
                    .ok_or_else(|| TypeError::new(
                        format!("no field '{}' on struct '{}'", field, struct_name), line,
                    ))
            }
            Type::Array(_) if field == "length" => Ok(Type::I32),
            other => Err(TypeError::new(
                format!("cannot access field '{}' on {:?}", field, other), line,
            )),
        }
    }
 
    // ─── Type assertions ──────────────────────────────────────────────────────
 
    fn assert_type(&self, got: &Type, expected: &Type, line: usize) -> TypeResult<()> {
        if got == expected { Ok(()) }
        else { Err(TypeError::new(format!("expected {:?}, got {:?}", expected, got), line)) }
    }
 
    fn assert_same(&self, a: &Type, b: &Type, line: usize) -> TypeResult<()> {
        if a == b { Ok(()) }
        else { Err(TypeError::new(format!("type mismatch: {:?} vs {:?}", a, b), line)) }
    }
 
    /// T is assignable to ?T; none (?void) is assignable to any ?T.
    fn assert_assignable(&self, got: &Type, expected: &Type, line: usize) -> TypeResult<()> {
        if got == expected { return Ok(()); }
        if let Type::Optional(inner) = expected {
            if got == inner.as_ref() { return Ok(()); }
        }
        if matches!(got, Type::Optional(b) if **b == Type::Void) {
            if matches!(expected, Type::Optional(_)) { return Ok(()); }
        }
        Err(TypeError::new(format!("cannot assign {:?} to {:?}", got, expected), line))
    }
 
    fn assert_numeric(&self, ty: &Type, line: usize) -> TypeResult<()> {
        match ty {
            Type::I32 | Type::I64 | Type::F64 => Ok(()),
            other => Err(TypeError::new(format!("expected numeric type, got {:?}", other), line)),
        }
    }
 
    fn assert_castable(&self, from: &Type, to: &Type, line: usize) -> TypeResult<()> {
        let numeric = |t: &Type| matches!(t, Type::I32 | Type::I64 | Type::F64);
        if numeric(from) && numeric(to) { return Ok(()); }
        if *from == Type::Bool && numeric(to) { return Ok(()); }
        Err(TypeError::new(format!("cannot cast {:?} to {:?}", from, to), line))
    }
}
 
