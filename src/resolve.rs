use crate::diagnostic::{Diagnostic, DiagnosticKind, Warning, WarningKind};
use crate::types::*;
use std::collections::HashMap;

type ResolveResult<T> = Result<T, Diagnostic>;

#[derive(Clone)]
struct FunctionSignature {
    return_type: CType,
    return_full_type: Option<FullType>,
    param_full_types: Vec<FullType>,
    variadic: bool,
    noreturn: bool,
}

impl PartialEq for FunctionSignature {
    fn eq(&self, other: &Self) -> bool {
        self.return_type == other.return_type
            && self.return_full_type == other.return_full_type
            && self.param_full_types == other.param_full_types
            && self.variadic == other.variadic
    }
}

impl FunctionSignature {
    fn from_decl(fd: &FunctionDeclaration) -> Self {
        let param_full_types = if fd.param_full_types.is_empty() {
            fd.params
                .iter()
                .map(|(_, t, pi)| FullType::from_decl(*t, *pi, &None))
                .collect()
        } else {
            fd.param_full_types.clone()
        };
        Self {
            return_type: fd.return_type,
            return_full_type: fd.return_full_type.clone(),
            param_full_types,
            variadic: fd.variadic,
            noreturn: fd.noreturn,
        }
    }
}

struct Resolver {
    scopes: Vec<HashMap<String, String>>,
    tag_scopes: Vec<HashMap<String, String>>,
    tag_counter: usize,
    var_counter: usize,
    loop_counter: usize,
    break_labels: Vec<String>,
    continue_labels: Vec<String>,
    functions: HashMap<String, FunctionSignature>,
    defined_labels: Vec<String>,
    goto_targets: Vec<String>,
    case_counter: usize,
    switch_depth: usize,
    warnings: Vec<Warning>,
}

pub struct ResolveOutput {
    pub program: Program,
    pub warnings: Vec<Warning>,
}

#[derive(Clone, Copy)]
enum BlockTerminator {
    Return,
    Break,
    Continue,
    Unreachable,
    NoreturnCall,
}

impl BlockTerminator {
    fn as_str(self) -> &'static str {
        match self {
            BlockTerminator::Return => "return",
            BlockTerminator::Break => "break",
            BlockTerminator::Continue => "continue",
            BlockTerminator::Unreachable => "__builtin_unreachable",
            BlockTerminator::NoreturnCall => "noreturn call",
        }
    }
}

impl Resolver {
    fn new() -> Self {
        Resolver {
            scopes: vec![HashMap::new()],
            tag_scopes: vec![HashMap::new()],
            tag_counter: 0,
            var_counter: 0,
            loop_counter: 0,
            break_labels: Vec::new(),
            continue_labels: Vec::new(),
            functions: HashMap::new(),
            defined_labels: Vec::new(),
            goto_targets: Vec::new(),
            case_counter: 0,
            switch_depth: 0,
            warnings: Vec::new(),
        }
    }

    fn warn_unreachable_statement(&mut self, after: BlockTerminator) {
        self.warnings
            .push(Warning::resolve(WarningKind::UnreachableStatement {
                after: after.as_str().to_string(),
            }));
    }

    fn warn_missing_return(&mut self, function: &str) {
        self.warnings
            .push(Warning::resolve(WarningKind::MissingReturn {
                function: function.to_string(),
            }));
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.tag_scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.tag_scopes.pop();
    }

    fn internal_error(message: impl Into<String>) -> Diagnostic {
        Diagnostic::resolve(DiagnosticKind::ResolveError {
            message: message.into(),
        })
    }

    fn current_scope_mut(&mut self) -> ResolveResult<&mut HashMap<String, String>> {
        self.scopes
            .last_mut()
            .ok_or_else(|| Self::internal_error("resolver scope stack is empty"))
    }

    fn current_tag_scope_mut(&mut self) -> ResolveResult<&mut HashMap<String, String>> {
        self.tag_scopes
            .last_mut()
            .ok_or_else(|| Self::internal_error("resolver tag scope stack is empty"))
    }

    fn declare_tag(&mut self, tag: &str) -> ResolveResult<String> {
        let existing = {
            let current = self.current_tag_scope_mut()?;
            current.get(tag).cloned()
        };
        if let Some(existing) = existing {
            return Ok(existing); // Redeclaration in same scope reuses ID
        }
        let unique = format!("{}.tag.{}", tag, self.tag_counter);
        self.tag_counter += 1;
        self.current_tag_scope_mut()?
            .insert(tag.to_string(), unique.clone());
        Ok(unique)
    }

    fn declare_var(&mut self, name: &str) -> ResolveResult<String> {
        if self.current_scope_mut()?.contains_key(name) {
            return Err(Diagnostic::resolve(DiagnosticKind::DuplicateVariable {
                name: name.to_string(),
            }));
        }
        let unique = format!("{}.{}", name, self.var_counter);
        self.var_counter += 1;
        self.current_scope_mut()?
            .insert(name.to_string(), unique.clone());
        Ok(unique)
    }

    fn declare_extern_var(&mut self, name: &str) -> ResolveResult<()> {
        self.current_scope_mut()?
            .insert(name.to_string(), name.to_string());
        Ok(())
    }

    fn declare_global_var(&mut self, name: &str) -> ResolveResult<()> {
        self.scopes
            .first_mut()
            .ok_or_else(|| Self::internal_error("resolver global scope is missing"))?
            .insert(name.to_string(), name.to_string());
        Ok(())
    }

    fn resolve_tag(&self, tag: &str) -> String {
        for scope in self.tag_scopes.iter().rev() {
            if let Some(unique) = scope.get(tag) {
                return unique.clone();
            }
        }
        // Not found — return as-is (will be caught later)
        tag.to_string()
    }

    fn resolve_var(&self, name: &str) -> ResolveResult<String> {
        for scope in self.scopes.iter().rev() {
            if let Some(unique) = scope.get(name) {
                return Ok(unique.clone());
            }
        }
        // Also check function names (for function-to-pointer decay: &func_name)
        if self.functions.contains_key(name) || name.starts_with("__builtin_") {
            return Ok(name.to_string());
        }
        Err(Diagnostic::resolve(DiagnosticKind::UndeclaredVariable {
            name: name.to_string(),
        }))
    }

    fn make_loop_label(&mut self) -> String {
        let label = format!("loop.{}", self.loop_counter);
        self.loop_counter += 1;
        label
    }

    fn current_break_label(&self) -> ResolveResult<String> {
        self.break_labels
            .last()
            .cloned()
            .ok_or_else(|| Diagnostic::resolve(DiagnosticKind::BreakOutsideLoopOrSwitch))
    }

    fn current_continue_label(&self) -> ResolveResult<String> {
        self.continue_labels
            .last()
            .cloned()
            .ok_or_else(|| Diagnostic::resolve(DiagnosticKind::ContinueOutsideLoop))
    }

    fn make_case_label(&mut self) -> String {
        let label = format!("case.{}", self.case_counter);
        self.case_counter += 1;
        label
    }

    fn resolve_struct_tags_in_ft(&self, ft: FullType) -> FullType {
        match ft {
            FullType::Struct(tag) => FullType::Struct(self.resolve_tag(&tag)),
            FullType::Pointer(inner) => {
                FullType::Pointer(Box::new(self.resolve_struct_tags_in_ft(*inner)))
            }
            FullType::Function {
                return_type,
                params,
                variadic,
            } => FullType::Function {
                return_type: Box::new(self.resolve_struct_tags_in_ft(*return_type)),
                params: params
                    .into_iter()
                    .map(|p| self.resolve_struct_tags_in_ft(p))
                    .collect(),
                variadic,
            },
            FullType::Array { elem, size } => FullType::Array {
                elem: Box::new(self.resolve_struct_tags_in_ft(*elem)),
                size,
            },
            other => other,
        }
    }

    fn resolve_exp(&mut self, exp: Exp) -> ResolveResult<Exp> {
        Ok(match exp {
            Exp::Constant(_)
            | Exp::LongConstant(_)
            | Exp::UIntConstant(_)
            | Exp::ULongConstant(_)
            | Exp::DoubleConstant(_)
            | Exp::StringLiteral(_)
            | Exp::Unreachable
            | Exp::AtomicFence => exp,
            Exp::AtomicFetch {
                op,
                ptr,
                arg,
                return_old,
            } => Exp::AtomicFetch {
                op,
                ptr: Box::new(self.resolve_exp(*ptr)?),
                arg: Box::new(self.resolve_exp(*arg)?),
                return_old,
            },
            Exp::AtomicExchange { ptr, value } => Exp::AtomicExchange {
                ptr: Box::new(self.resolve_exp(*ptr)?),
                value: Box::new(self.resolve_exp(*value)?),
            },
            Exp::AtomicCompareExchange {
                ptr,
                expected,
                desired,
            } => Exp::AtomicCompareExchange {
                ptr: Box::new(self.resolve_exp(*ptr)?),
                expected: Box::new(self.resolve_exp(*expected)?),
                desired: Box::new(self.resolve_exp(*desired)?),
            },
            Exp::AtomicCompareSwap {
                ptr,
                expected,
                desired,
                return_old,
            } => Exp::AtomicCompareSwap {
                ptr: Box::new(self.resolve_exp(*ptr)?),
                expected: Box::new(self.resolve_exp(*expected)?),
                desired: Box::new(self.resolve_exp(*desired)?),
                return_old,
            },
            Exp::Subscript(arr, idx) => Exp::Subscript(
                Box::new(self.resolve_exp(*arr)?),
                Box::new(self.resolve_exp(*idx)?),
            ),
            Exp::ArrayInit(elems) => {
                let elems = elems
                    .into_iter()
                    .map(|e| self.resolve_exp(e))
                    .collect::<ResolveResult<Vec<_>>>()?;
                Exp::ArrayInit(elems)
            }
            Exp::DesignatedInit(designators, value) => {
                let designators = designators
                    .into_iter()
                    .map(|designator| match designator {
                        Designator::Field(name) => Ok(Designator::Field(name)),
                        Designator::Index(index) => {
                            Ok(Designator::Index(Box::new(self.resolve_exp(*index)?)))
                        }
                    })
                    .collect::<ResolveResult<Vec<_>>>()?;
                Exp::DesignatedInit(designators, Box::new(self.resolve_exp(*value)?))
            }
            Exp::Var(name) => Exp::Var(self.resolve_var(&name)?),
            Exp::Cast(t, ft, inner) => {
                let resolved_ft = ft.map(|f| self.resolve_struct_tags_in_ft(f));
                Exp::Cast(t, resolved_ft, Box::new(self.resolve_exp(*inner)?))
            }
            Exp::Unary(op, inner) => Exp::Unary(op, Box::new(self.resolve_exp(*inner)?)),
            Exp::Binary(op, left, right) => Exp::Binary(
                op,
                Box::new(self.resolve_exp(*left)?),
                Box::new(self.resolve_exp(*right)?),
            ),
            Exp::Assign(left, right) => Exp::Assign(
                Box::new(self.resolve_exp(*left)?),
                Box::new(self.resolve_exp(*right)?),
            ),
            Exp::CompoundAssign(op, left, right) => Exp::CompoundAssign(
                op,
                Box::new(self.resolve_exp(*left)?),
                Box::new(self.resolve_exp(*right)?),
            ),
            Exp::Conditional(cond, then_exp, else_exp) => Exp::Conditional(
                Box::new(self.resolve_exp(*cond)?),
                Box::new(self.resolve_exp(*then_exp)?),
                Box::new(self.resolve_exp(*else_exp)?),
            ),
            Exp::FunctionCall(name, args) => {
                let resolved_args = args
                    .into_iter()
                    .map(|a| self.resolve_exp(a))
                    .collect::<ResolveResult<Vec<_>>>()?;
                if self.functions.contains_key(&name) || name.starts_with("__builtin_") {
                    Exp::FunctionCall(name, resolved_args)
                } else {
                    // Could be an indirect call through a function pointer variable
                    let resolved_name = self.resolve_var(&name)?;
                    Exp::FunctionCall(resolved_name, resolved_args)
                }
            }
            Exp::SizeOf(inner) => Exp::SizeOf(Box::new(self.resolve_exp(*inner)?)),
            Exp::SizeOfType(ct, ft) => Exp::SizeOfType(ct, self.resolve_struct_tags_in_ft(ft)),
            Exp::AlignOfType(ft) => Exp::AlignOfType(self.resolve_struct_tags_in_ft(ft)),
            Exp::Dot(inner, member) => Exp::Dot(Box::new(self.resolve_exp(*inner)?), member),
            Exp::Arrow(inner, member) => Exp::Arrow(Box::new(self.resolve_exp(*inner)?), member),
            Exp::Comma(left, right) => Exp::Comma(
                Box::new(self.resolve_exp(*left)?),
                Box::new(self.resolve_exp(*right)?),
            ),
            Exp::StatementExpr(block, result, result_type) => {
                self.push_scope();
                let resolved_block = self.resolve_block(block)?;
                let resolved_result = result
                    .map(|exp| self.resolve_exp(*exp).map(Box::new))
                    .transpose()?;
                self.pop_scope();
                Exp::StatementExpr(
                    resolved_block,
                    resolved_result,
                    result_type.map(|ft| self.resolve_struct_tags_in_ft(ft)),
                )
            }
            Exp::IndirectCall(callee, args) => {
                let resolved_callee = self.resolve_exp(*callee)?;
                let resolved_args = args
                    .into_iter()
                    .map(|a| self.resolve_exp(a))
                    .collect::<ResolveResult<Vec<_>>>()?;
                Exp::IndirectCall(Box::new(resolved_callee), resolved_args)
            }
        })
    }

    fn resolve_statement(&mut self, stmt: Statement) -> ResolveResult<Statement> {
        Ok(match stmt {
            Statement::Return(exp) => {
                Statement::Return(exp.map(|e| self.resolve_exp(e)).transpose()?)
            }
            Statement::Expression(exp) => Statement::Expression(self.resolve_exp(exp)?),
            Statement::If(cond, then_stmt, else_stmt) => Statement::If(
                self.resolve_exp(cond)?,
                Box::new(self.resolve_statement(*then_stmt)?),
                else_stmt
                    .map(|s| self.resolve_statement(*s).map(Box::new))
                    .transpose()?,
            ),
            Statement::Block(block) => {
                self.push_scope();
                let resolved = self.resolve_block(block)?;
                self.pop_scope();
                Statement::Block(resolved)
            }
            Statement::While {
                condition, body, ..
            } => {
                let label = self.make_loop_label();
                self.break_labels.push(label.clone());
                self.continue_labels.push(label.clone());
                let resolved = Statement::While {
                    condition: self.resolve_exp(condition)?,
                    body: Box::new(self.resolve_statement(*body)?),
                    label,
                };
                self.break_labels.pop();
                self.continue_labels.pop();
                resolved
            }
            Statement::DoWhile {
                body, condition, ..
            } => {
                let label = self.make_loop_label();
                self.break_labels.push(label.clone());
                self.continue_labels.push(label.clone());
                let resolved = Statement::DoWhile {
                    body: Box::new(self.resolve_statement(*body)?),
                    condition: self.resolve_exp(condition)?,
                    label,
                };
                self.break_labels.pop();
                self.continue_labels.pop();
                resolved
            }
            Statement::For {
                init,
                condition,
                post,
                body,
                ..
            } => {
                let has_decl = matches!(&*init, ForInit::Declaration(_));
                if has_decl {
                    self.push_scope();
                }
                let resolved_init = match *init {
                    ForInit::Declaration(vd) => ForInit::Declaration(self.resolve_var_decl(vd)?),
                    ForInit::Expression(opt_exp) => {
                        ForInit::Expression(opt_exp.map(|e| self.resolve_exp(e)).transpose()?)
                    }
                };
                let resolved_cond = condition.map(|e| self.resolve_exp(e)).transpose()?;
                let resolved_post = post.map(|e| self.resolve_exp(e)).transpose()?;
                let label = self.make_loop_label();
                self.break_labels.push(label.clone());
                self.continue_labels.push(label.clone());
                let resolved_body = Box::new(self.resolve_statement(*body)?);
                self.break_labels.pop();
                self.continue_labels.pop();
                if has_decl {
                    self.pop_scope();
                }
                Statement::For {
                    init: Box::new(resolved_init),
                    condition: resolved_cond,
                    post: resolved_post,
                    body: resolved_body,
                    label,
                }
            }
            Statement::Break(_) => Statement::Break(self.current_break_label()?),
            Statement::Continue(_) => Statement::Continue(self.current_continue_label()?),
            Statement::Goto(label) => {
                self.goto_targets.push(label.clone());
                Statement::Goto(label)
            }
            Statement::Label(name, body) => {
                if self.defined_labels.contains(&name) {
                    return Err(Diagnostic::resolve(DiagnosticKind::DuplicateLabel { name }));
                }
                self.defined_labels.push(name.clone());
                Statement::Label(name, Box::new(self.resolve_statement(*body)?))
            }
            Statement::Switch { control, body, .. } => {
                let label = format!("switch.{}", self.loop_counter);
                self.loop_counter += 1;
                self.break_labels.push(label.clone());
                self.switch_depth += 1;
                let resolved_control = self.resolve_exp(control)?;
                let resolved_body = Box::new(self.resolve_statement(*body)?);
                self.break_labels.pop();
                self.switch_depth -= 1;
                let mut cases = Vec::new();
                collect_cases(&resolved_body, &mut cases)?;
                Statement::Switch {
                    control: resolved_control,
                    body: resolved_body,
                    label,
                    cases,
                }
            }
            Statement::Case { value, body, .. } => {
                if self.switch_depth == 0 {
                    return Err(Diagnostic::resolve(DiagnosticKind::CaseOutsideSwitch));
                }
                let label = self.make_case_label();
                Statement::Case {
                    value: self.resolve_exp(value)?,
                    body: Box::new(self.resolve_statement(*body)?),
                    label,
                }
            }
            Statement::Default { body, .. } => {
                if self.switch_depth == 0 {
                    return Err(Diagnostic::resolve(DiagnosticKind::DefaultOutsideSwitch));
                }
                let label = self.make_case_label();
                Statement::Default {
                    body: Box::new(self.resolve_statement(*body)?),
                    label,
                }
            }
            Statement::Null => Statement::Null,
        })
    }

    fn resolve_var_decl(&mut self, vd: VarDeclaration) -> ResolveResult<VarDeclaration> {
        if vd
            .storage_class
            .as_ref()
            .is_some_and(StorageClass::is_extern)
        {
            self.declare_extern_var(&vd.name)?;
            let resolved_dft = vd
                .decl_full_type
                .map(|ft| self.resolve_struct_tags_in_ft(ft));
            return Ok(VarDeclaration {
                decl_full_type: resolved_dft,
                ..vd
            });
        }
        let unique_name = self.declare_var(&vd.name)?;
        let init = vd.init.map(|e| self.resolve_exp(e)).transpose()?;
        let resolved_dft = vd
            .decl_full_type
            .map(|ft| self.resolve_struct_tags_in_ft(ft));
        Ok(VarDeclaration {
            name: unique_name,
            var_type: vd.var_type,
            ptr_info: vd.ptr_info,
            array_dims: vd.array_dims,
            decl_full_type: resolved_dft,
            init,
            storage_class: vd.storage_class,
            alignment: vd.alignment,
        })
    }

    fn resolve_block(&mut self, block: Block) -> ResolveResult<Block> {
        let mut resolved = Vec::with_capacity(block.len());
        let mut unreachable_after = None;

        for item in block {
            let resolved_item = match item {
                BlockItem::Declaration(Declaration::VarDecl(vd)) => {
                    BlockItem::Declaration(Declaration::VarDecl(self.resolve_var_decl(vd)?))
                }
                BlockItem::Declaration(Declaration::FunDecl(fd)) => {
                    let signature = FunctionSignature::from_decl(&fd);
                    if let Some(existing) = self.functions.get(&fd.name) {
                        if existing != &signature {
                            return Err(Diagnostic::resolve(
                                DiagnosticKind::ConflictingFunctionParameterCount {
                                    name: fd.name.clone(),
                                },
                            ));
                        }
                    }
                    let mut signature = signature;
                    signature.noreturn = signature.noreturn
                        || self
                            .functions
                            .get(&fd.name)
                            .is_some_and(|existing| existing.noreturn);
                    self.functions.insert(fd.name.clone(), signature);
                    let resolved_rft = fd
                        .return_full_type
                        .map(|ft| self.resolve_struct_tags_in_ft(ft));
                    let resolved_pfts: Vec<FullType> = fd
                        .param_full_types
                        .into_iter()
                        .map(|ft| self.resolve_struct_tags_in_ft(ft))
                        .collect();
                    BlockItem::Declaration(Declaration::FunDecl(FunctionDeclaration {
                        return_full_type: resolved_rft,
                        param_full_types: resolved_pfts,
                        ..fd
                    }))
                }
                BlockItem::Declaration(Declaration::StructDecl(sd)) => {
                    let unique_tag = self.declare_tag(&sd.tag)?;
                    let resolved_members: Vec<MemberDeclaration> = sd
                        .members
                        .into_iter()
                        .map(|m| {
                            let resolved_ft = self.resolve_struct_tags_in_ft(m.member_full_type);
                            MemberDeclaration {
                                name: m.name,
                                member_type: m.member_type,
                                member_full_type: resolved_ft,
                                bit_width: m.bit_width,
                                alignment: m.alignment,
                            }
                        })
                        .collect();
                    BlockItem::Declaration(Declaration::StructDecl(StructDeclaration {
                        tag: unique_tag,
                        members: resolved_members,
                        is_union: sd.is_union,
                        packed: sd.packed,
                    }))
                }
                BlockItem::Declaration(Declaration::TypedefDecl) => {
                    BlockItem::Declaration(Declaration::TypedefDecl)
                }
                BlockItem::Statement(stmt) => {
                    let is_label_entry = matches!(
                        &stmt,
                        Statement::Label(_, _) | Statement::Case { .. } | Statement::Default { .. }
                    );
                    if let Some(after) = unreachable_after {
                        if is_label_entry {
                            unreachable_after = None;
                        } else {
                            self.warn_unreachable_statement(after);
                            unreachable_after = None;
                        }
                    }
                    BlockItem::Statement(self.resolve_statement(stmt)?)
                }
            };

            unreachable_after = match &resolved_item {
                BlockItem::Statement(Statement::Return(_)) => Some(BlockTerminator::Return),
                BlockItem::Statement(Statement::Break(_)) => Some(BlockTerminator::Break),
                BlockItem::Statement(Statement::Continue(_)) => Some(BlockTerminator::Continue),
                BlockItem::Statement(Statement::Expression(Exp::Unreachable)) => {
                    Some(BlockTerminator::Unreachable)
                }
                BlockItem::Statement(Statement::Expression(Exp::FunctionCall(name, _)))
                    if self.functions.get(name).is_some_and(|sig| sig.noreturn) =>
                {
                    Some(BlockTerminator::NoreturnCall)
                }
                BlockItem::Statement(Statement::Label(_, _))
                | BlockItem::Statement(Statement::Case { .. })
                | BlockItem::Statement(Statement::Default { .. }) => None,
                _ => unreachable_after,
            };
            resolved.push(resolved_item);
        }

        Ok(resolved)
    }

    fn resolve_function(
        &mut self,
        func: FunctionDeclaration,
    ) -> ResolveResult<FunctionDeclaration> {
        match func.body {
            None => {
                // Resolve struct tags in prototypes too
                let resolved_rft = func
                    .return_full_type
                    .map(|ft| self.resolve_struct_tags_in_ft(ft));
                let resolved_pfts: Vec<FullType> = func
                    .param_full_types
                    .into_iter()
                    .map(|ft| self.resolve_struct_tags_in_ft(ft))
                    .collect();
                Ok(FunctionDeclaration {
                    return_full_type: resolved_rft,
                    param_full_types: resolved_pfts,
                    ..func
                })
            }
            Some(body) => {
                self.push_scope();
                self.defined_labels.clear();
                self.goto_targets.clear();
                let mut resolved_params = Vec::new();
                for (name, ptype, pi) in &func.params {
                    resolved_params.push((self.declare_var(name)?, *ptype, *pi));
                }
                let resolved_body = self.resolve_block(body)?;
                if func.return_type != CType::Void
                    && !block_guarantees_return(&resolved_body, &self.functions)
                {
                    self.warn_missing_return(&func.name);
                }
                self.pop_scope();
                for target in &self.goto_targets {
                    if !self.defined_labels.contains(target) {
                        return Err(Diagnostic::resolve(DiagnosticKind::UndefinedGotoLabel {
                            name: target.clone(),
                        }));
                    }
                }
                let resolved_rft = func
                    .return_full_type
                    .map(|ft| self.resolve_struct_tags_in_ft(ft));
                let resolved_pfts: Vec<FullType> = func
                    .param_full_types
                    .into_iter()
                    .map(|ft| self.resolve_struct_tags_in_ft(ft))
                    .collect();
                Ok(FunctionDeclaration {
                    name: func.name,
                    return_type: func.return_type,
                    return_ptr_info: func.return_ptr_info,
                    return_full_type: resolved_rft,
                    params: resolved_params,
                    body: Some(resolved_body),
                    storage_class: func.storage_class,
                    param_full_types: resolved_pfts,
                    variadic: func.variadic,
                    noreturn: func.noreturn,
                })
            }
        }
    }
}

fn collect_cases(stmt: &Statement, cases: &mut Vec<SwitchCase>) -> ResolveResult<()> {
    match stmt {
        Statement::Case { value, body, label } => {
            let val = eval_integer_constant_exp(value)
                .ok_or_else(|| Diagnostic::resolve(DiagnosticKind::NonConstantCaseValue))?;
            cases.push(SwitchCase {
                value: Some(val),
                label: label.clone(),
            });
            collect_cases(body, cases)?;
        }
        Statement::Default { body, label } => {
            cases.push(SwitchCase {
                value: None,
                label: label.clone(),
            });
            collect_cases(body, cases)?;
        }
        Statement::Block(items) => {
            for item in items {
                if let BlockItem::Statement(s) = item {
                    collect_cases(s, cases)?;
                }
            }
        }
        Statement::If(_, then_s, else_s) => {
            collect_cases(then_s, cases)?;
            if let Some(e) = else_s {
                collect_cases(e, cases)?;
            }
        }
        Statement::While { body, .. } => collect_cases(body, cases)?,
        Statement::DoWhile { body, .. } => collect_cases(body, cases)?,
        Statement::For { body, .. } => collect_cases(body, cases)?,
        Statement::Label(_, body) => collect_cases(body, cases)?,
        Statement::Switch { .. } => {}
        _ => {}
    }
    Ok(())
}

fn block_guarantees_return(
    block: &[BlockItem],
    functions: &HashMap<String, FunctionSignature>,
) -> bool {
    block.iter().rev().any(|item| match item {
        BlockItem::Statement(stmt) => statement_guarantees_return(stmt, functions),
        _ => false,
    })
}

fn statement_guarantees_return(
    stmt: &Statement,
    functions: &HashMap<String, FunctionSignature>,
) -> bool {
    match stmt {
        Statement::Return(_) => true,
        Statement::Expression(Exp::Unreachable) => true,
        Statement::Expression(Exp::FunctionCall(name, _)) => {
            functions.get(name).is_some_and(|sig| sig.noreturn)
        }
        Statement::Block(block) => block_guarantees_return(block, functions),
        Statement::If(_, then_stmt, Some(else_stmt)) => {
            statement_guarantees_return(then_stmt, functions)
                && statement_guarantees_return(else_stmt, functions)
        }
        Statement::Label(_, body)
        | Statement::Case { body, .. }
        | Statement::Default { body, .. } => statement_guarantees_return(body, functions),
        _ => false,
    }
}

fn eval_integer_constant_exp(exp: &Exp) -> Option<i64> {
    match exp {
        Exp::Constant(c) | Exp::LongConstant(c) | Exp::UIntConstant(c) | Exp::ULongConstant(c) => {
            Some(*c)
        }
        Exp::DoubleConstant(d) => Some(*d as i64),
        Exp::Cast(_, _, inner) => eval_integer_constant_exp(inner),
        Exp::Unary(op, inner) => {
            let value = eval_integer_constant_exp(inner)?;
            match op {
                UnaryOp::Negate => Some(-value),
                UnaryOp::Complement => Some(!value),
                UnaryOp::LogicalNot => Some((value == 0) as i64),
                _ => None,
            }
        }
        Exp::Binary(op, left, right) => {
            let left = eval_integer_constant_exp(left)?;
            let right = eval_integer_constant_exp(right)?;
            match op {
                BinaryOp::Add => Some(left + right),
                BinaryOp::Sub => Some(left - right),
                BinaryOp::Mul => Some(left * right),
                BinaryOp::Div => (right != 0).then_some(left / right),
                BinaryOp::Mod => (right != 0).then_some(left % right),
                BinaryOp::BitwiseAnd => Some(left & right),
                BinaryOp::BitwiseNand => Some(!(left & right)),
                BinaryOp::BitwiseOr => Some(left | right),
                BinaryOp::BitwiseXor => Some(left ^ right),
                BinaryOp::ShiftLeft => u32::try_from(right)
                    .ok()
                    .and_then(|amount| left.checked_shl(amount)),
                BinaryOp::ShiftRight => u32::try_from(right)
                    .ok()
                    .and_then(|amount| left.checked_shr(amount)),
                BinaryOp::LogicalAnd => Some((left != 0 && right != 0) as i64),
                BinaryOp::LogicalOr => Some((left != 0 || right != 0) as i64),
                BinaryOp::Equal => Some((left == right) as i64),
                BinaryOp::NotEqual => Some((left != right) as i64),
                BinaryOp::LessThan => Some((left < right) as i64),
                BinaryOp::GreaterThan => Some((left > right) as i64),
                BinaryOp::LessEqual => Some((left <= right) as i64),
                BinaryOp::GreaterEqual => Some((left >= right) as i64),
            }
        }
        Exp::Conditional(cond, then_exp, else_exp) => {
            if eval_integer_constant_exp(cond)? != 0 {
                eval_integer_constant_exp(then_exp)
            } else {
                eval_integer_constant_exp(else_exp)
            }
        }
        _ => None,
    }
}

pub fn resolve(program: Program) -> ResolveResult<ResolveOutput> {
    let mut resolver = Resolver::new();

    for decl in &program.declarations {
        match decl {
            Declaration::FunDecl(fd) => {
                let signature = FunctionSignature::from_decl(fd);
                if let Some(existing) = resolver.functions.get(&fd.name) {
                    if existing != &signature {
                        return Err(Diagnostic::resolve(
                            DiagnosticKind::ConflictingFunctionParameterCount {
                                name: fd.name.clone(),
                            },
                        ));
                    }
                }
                let mut signature = signature;
                signature.noreturn = signature.noreturn
                    || resolver
                        .functions
                        .get(&fd.name)
                        .is_some_and(|existing| existing.noreturn);
                resolver.functions.insert(fd.name.clone(), signature);
            }
            Declaration::VarDecl(vd) => {
                resolver.declare_global_var(&vd.name)?;
            }
            Declaration::StructDecl(sd) => {
                resolver.declare_tag(&sd.tag)?;
            }
            Declaration::TypedefDecl => {}
        }
    }

    let declarations = program
        .declarations
        .into_iter()
        .map(|decl| {
            Ok(match decl {
                Declaration::FunDecl(fd) => Declaration::FunDecl(resolver.resolve_function(fd)?),
                Declaration::VarDecl(vd) => {
                    let init = vd.init.map(|e| resolver.resolve_exp(e)).transpose()?;
                    let resolved_dft = vd
                        .decl_full_type
                        .map(|ft| resolver.resolve_struct_tags_in_ft(ft));
                    Declaration::VarDecl(VarDeclaration {
                        name: vd.name,
                        var_type: vd.var_type,
                        ptr_info: vd.ptr_info,
                        array_dims: vd.array_dims,
                        decl_full_type: resolved_dft,
                        init,
                        storage_class: vd.storage_class,
                        alignment: vd.alignment,
                    })
                }
                Declaration::StructDecl(sd) => {
                    let unique_tag = resolver.resolve_tag(&sd.tag);
                    let resolved_members: Vec<MemberDeclaration> = sd
                        .members
                        .into_iter()
                        .map(|m| {
                            let resolved_ft =
                                resolver.resolve_struct_tags_in_ft(m.member_full_type);
                            MemberDeclaration {
                                name: m.name,
                                member_type: m.member_type,
                                member_full_type: resolved_ft,
                                bit_width: m.bit_width,
                                alignment: m.alignment,
                            }
                        })
                        .collect();
                    Declaration::StructDecl(StructDeclaration {
                        tag: unique_tag,
                        members: resolved_members,
                        is_union: sd.is_union,
                        packed: sd.packed,
                    })
                }
                Declaration::TypedefDecl => Declaration::TypedefDecl,
            })
        })
        .collect::<ResolveResult<Vec<_>>>()?;

    Ok(ResolveOutput {
        program: Program { declarations },
        warnings: resolver.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lex, parse};

    fn warnings_for(source: &str) -> Result<Vec<Warning>, String> {
        let tokens = lex::lex(source)?;
        let ast = parse::parse(tokens)?;
        Ok(resolve(ast).map_err(|err| err.render())?.warnings)
    }

    fn require_err<T>(result: ResolveResult<T>, context: &str) -> ResolveResult<Diagnostic> {
        match result {
            Ok(_) => Err(Diagnostic::resolve(DiagnosticKind::ResolveError {
                message: format!("{context} unexpectedly succeeded"),
            })),
            Err(err) => Ok(err),
        }
    }

    #[test]
    fn warns_on_unreachable_statement_after_return_in_block() -> Result<(), String> {
        let warnings = warnings_for("int main(void) { return 0; 1 + 2; }\n")?;

        assert_eq!(warnings.len(), 1);
        assert_eq!(
            warnings[0].kind,
            WarningKind::UnreachableStatement {
                after: "return".to_string()
            }
        );
        assert_eq!(
            warnings[0].render(),
            "resolve warning: unreachable statement after return"
        );
        Ok(())
    }

    #[test]
    fn warns_on_unreachable_statement_after_loop_control_in_block() -> Result<(), String> {
        let warnings = warnings_for(
            "int main(void) { while (1) { break; 1 + 2; continue; 3 + 4; } return 0; }\n",
        )?;

        assert_eq!(warnings.len(), 2);
        assert_eq!(
            warnings[0].kind,
            WarningKind::UnreachableStatement {
                after: "break".to_string()
            }
        );
        assert_eq!(
            warnings[1].kind,
            WarningKind::UnreachableStatement {
                after: "continue".to_string()
            }
        );
        Ok(())
    }

    #[test]
    fn does_not_warn_on_label_entry_after_return() -> Result<(), String> {
        let warnings = warnings_for("int main(void) { goto again; return 0; again: return 1; }\n")?;

        assert!(warnings.is_empty());
        Ok(())
    }

    #[test]
    fn resolve_returns_warnings_with_program() -> Result<(), String> {
        let tokens = lex::lex("int main(void) { return 0; 1 + 2; }\n")?;
        let ast = parse::parse(tokens)?;

        let output = resolve(ast).map_err(|err| err.render())?;
        assert_eq!(output.warnings.len(), 1);
        Ok(())
    }

    #[test]
    fn empty_scope_stack_reports_structured_resolve_error() -> ResolveResult<()> {
        let mut resolver = Resolver::new();
        resolver.scopes.clear();

        let err = require_err(
            resolver.declare_var("x"),
            "missing scope should be an error",
        )?;

        assert_eq!(
            err.kind,
            DiagnosticKind::ResolveError {
                message: "resolver scope stack is empty".to_string()
            }
        );
        Ok(())
    }

    #[test]
    fn empty_tag_scope_stack_reports_structured_resolve_error() -> ResolveResult<()> {
        let mut resolver = Resolver::new();
        resolver.tag_scopes.clear();

        let err = require_err(
            resolver.declare_tag("S"),
            "missing tag scope should be an error",
        )?;

        assert_eq!(
            err.kind,
            DiagnosticKind::ResolveError {
                message: "resolver tag scope stack is empty".to_string()
            }
        );
        Ok(())
    }

    #[test]
    fn warns_on_missing_return_in_non_void_function() -> Result<(), String> {
        let warnings = warnings_for("int f(int x) { if (x) { return 1; } }\n")?;

        assert_eq!(warnings.len(), 1);
        assert_eq!(
            warnings[0].kind,
            WarningKind::MissingReturn {
                function: "f".to_string()
            }
        );
        Ok(())
    }

    #[test]
    fn noreturn_direct_call_satisfies_missing_return_analysis() -> Result<(), String> {
        let warnings = warnings_for(
            "_Noreturn void die(void); int f(int x) { if (x) { return 1; } die(); }\n",
        )?;

        assert!(warnings.is_empty(), "{warnings:?}");
        Ok(())
    }

    #[test]
    fn warns_on_unreachable_statement_after_noreturn_direct_call() -> Result<(), String> {
        let warnings =
            warnings_for("_Noreturn void die(void); int f(void) { die(); return 1; }\n")?;

        assert_eq!(warnings.len(), 1);
        assert_eq!(
            warnings[0].kind,
            WarningKind::UnreachableStatement {
                after: "noreturn call".to_string()
            }
        );
        Ok(())
    }
}
