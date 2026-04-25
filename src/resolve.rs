use crate::types::*;
use std::collections::HashMap;

struct Resolver {
    scopes: Vec<HashMap<String, String>>,
    tag_scopes: Vec<HashMap<String, String>>,
    tag_counter: usize,
    var_counter: usize,
    loop_counter: usize,
    break_labels: Vec<String>,
    continue_labels: Vec<String>,
    functions: HashMap<String, usize>,
    defined_labels: Vec<String>,
    goto_targets: Vec<String>,
    case_counter: usize,
    switch_depth: usize,
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
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.tag_scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.tag_scopes.pop();
    }

    fn declare_tag(&mut self, tag: &str) -> String {
        let current = self.tag_scopes.last_mut().unwrap();
        if let Some(existing) = current.get(tag) {
            return existing.clone(); // Redeclaration in same scope reuses ID
        }
        let unique = format!("{}.tag.{}", tag, self.tag_counter);
        self.tag_counter += 1;
        current.insert(tag.to_string(), unique.clone());
        unique
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

    fn declare_var(&mut self, name: &str) -> String {
        let current = self.scopes.last_mut().unwrap();
        if current.contains_key(name) {
            panic!("Duplicate variable declaration: '{}'", name);
        }
        let unique = format!("{}.{}", name, self.var_counter);
        self.var_counter += 1;
        current.insert(name.to_string(), unique.clone());
        unique
    }

    fn resolve_var(&self, name: &str) -> String {
        for scope in self.scopes.iter().rev() {
            if let Some(unique) = scope.get(name) {
                return unique.clone();
            }
        }
        panic!("Undeclared variable: '{}'", name);
    }

    fn make_loop_label(&mut self) -> String {
        let label = format!("loop.{}", self.loop_counter);
        self.loop_counter += 1;
        label
    }

    fn current_break_label(&self) -> String {
        self.break_labels
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("break outside of loop or switch"))
    }

    fn current_continue_label(&self) -> String {
        self.continue_labels
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("continue outside of loop"))
    }

    fn make_case_label(&mut self) -> String {
        let label = format!("case.{}", self.case_counter);
        self.case_counter += 1;
        label
    }

    fn resolve_struct_tags_in_ft(&self, ft: FullType) -> FullType {
        match ft {
            FullType::Struct(tag) => FullType::Struct(self.resolve_tag(&tag)),
            FullType::Pointer(inner) => FullType::Pointer(Box::new(self.resolve_struct_tags_in_ft(*inner))),
            FullType::Array { elem, size } => FullType::Array { elem: Box::new(self.resolve_struct_tags_in_ft(*elem)), size },
            other => other,
        }
    }

    fn resolve_exp(&self, exp: Exp) -> Exp {
        match exp {
            Exp::Constant(_) | Exp::LongConstant(_) | Exp::UIntConstant(_) | Exp::ULongConstant(_) | Exp::DoubleConstant(_) | Exp::StringLiteral(_) => exp,
            Exp::Subscript(arr, idx) => Exp::Subscript(
                Box::new(self.resolve_exp(*arr)),
                Box::new(self.resolve_exp(*idx)),
            ),
            Exp::ArrayInit(elems) => Exp::ArrayInit(
                elems.into_iter().map(|e| self.resolve_exp(e)).collect(),
            ),
            Exp::Var(name) => Exp::Var(self.resolve_var(&name)),
            Exp::Cast(t, ft, inner) => {
                let resolved_ft = ft.map(|f| self.resolve_struct_tags_in_ft(f));
                Exp::Cast(t, resolved_ft, Box::new(self.resolve_exp(*inner)))
            }
            Exp::Unary(op, inner) => Exp::Unary(op, Box::new(self.resolve_exp(*inner))),
            Exp::Binary(op, left, right) => Exp::Binary(
                op,
                Box::new(self.resolve_exp(*left)),
                Box::new(self.resolve_exp(*right)),
            ),
            Exp::Assign(left, right) => Exp::Assign(
                Box::new(self.resolve_exp(*left)),
                Box::new(self.resolve_exp(*right)),
            ),
            Exp::CompoundAssign(op, left, right) => Exp::CompoundAssign(
                op,
                Box::new(self.resolve_exp(*left)),
                Box::new(self.resolve_exp(*right)),
            ),
            Exp::Conditional(cond, then_exp, else_exp) => Exp::Conditional(
                Box::new(self.resolve_exp(*cond)),
                Box::new(self.resolve_exp(*then_exp)),
                Box::new(self.resolve_exp(*else_exp)),
            ),
            Exp::FunctionCall(name, args) => {
                let resolved_args = args.into_iter().map(|a| self.resolve_exp(a)).collect();
                if !self.functions.contains_key(&name) {
                    panic!("Undeclared function: '{}'", name);
                }
                Exp::FunctionCall(name, resolved_args)
            }
            Exp::SizeOf(inner) => Exp::SizeOf(Box::new(self.resolve_exp(*inner))),
            Exp::SizeOfType(ct, ft) => Exp::SizeOfType(ct, self.resolve_struct_tags_in_ft(ft)),
            Exp::Dot(inner, member) => Exp::Dot(Box::new(self.resolve_exp(*inner)), member),
            Exp::Arrow(inner, member) => Exp::Arrow(Box::new(self.resolve_exp(*inner)), member),
        }
    }

    fn resolve_statement(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::Return(exp) => Statement::Return(exp.map(|e| self.resolve_exp(e))),
            Statement::Expression(exp) => Statement::Expression(self.resolve_exp(exp)),
            Statement::If(cond, then_stmt, else_stmt) => Statement::If(
                self.resolve_exp(cond),
                Box::new(self.resolve_statement(*then_stmt)),
                else_stmt.map(|s| Box::new(self.resolve_statement(*s))),
            ),
            Statement::Block(block) => {
                self.push_scope();
                let resolved = self.resolve_block(block);
                self.pop_scope();
                Statement::Block(resolved)
            }
            Statement::While { condition, body, .. } => {
                let label = self.make_loop_label();
                self.break_labels.push(label.clone());
                self.continue_labels.push(label.clone());
                let resolved = Statement::While {
                    condition: self.resolve_exp(condition),
                    body: Box::new(self.resolve_statement(*body)),
                    label,
                };
                self.break_labels.pop();
                self.continue_labels.pop();
                resolved
            }
            Statement::DoWhile { body, condition, .. } => {
                let label = self.make_loop_label();
                self.break_labels.push(label.clone());
                self.continue_labels.push(label.clone());
                let resolved = Statement::DoWhile {
                    body: Box::new(self.resolve_statement(*body)),
                    condition: self.resolve_exp(condition),
                    label,
                };
                self.break_labels.pop();
                self.continue_labels.pop();
                resolved
            }
            Statement::For { init, condition, post, body, .. } => {
                let has_decl = matches!(&init, ForInit::Declaration(_));
                if has_decl {
                    self.push_scope();
                }
                let resolved_init = match init {
                    ForInit::Declaration(vd) => ForInit::Declaration(self.resolve_var_decl(vd)),
                    ForInit::Expression(opt_exp) => {
                        ForInit::Expression(opt_exp.map(|e| self.resolve_exp(e)))
                    }
                };
                let resolved_cond = condition.map(|e| self.resolve_exp(e));
                let resolved_post = post.map(|e| self.resolve_exp(e));
                let label = self.make_loop_label();
                self.break_labels.push(label.clone());
                self.continue_labels.push(label.clone());
                let resolved_body = Box::new(self.resolve_statement(*body));
                self.break_labels.pop();
                self.continue_labels.pop();
                if has_decl {
                    self.pop_scope();
                }
                Statement::For {
                    init: resolved_init,
                    condition: resolved_cond,
                    post: resolved_post,
                    body: resolved_body,
                    label,
                }
            }
            Statement::Break(_) => Statement::Break(self.current_break_label()),
            Statement::Continue(_) => Statement::Continue(self.current_continue_label()),
            Statement::Goto(label) => {
                self.goto_targets.push(label.clone());
                Statement::Goto(label)
            }
            Statement::Label(name, body) => {
                if self.defined_labels.contains(&name) {
                    panic!("Duplicate label: '{}'", name);
                }
                self.defined_labels.push(name.clone());
                Statement::Label(name, Box::new(self.resolve_statement(*body)))
            }
            Statement::Switch { control, body, .. } => {
                let label = format!("switch.{}", self.loop_counter);
                self.loop_counter += 1;
                self.break_labels.push(label.clone());
                self.switch_depth += 1;
                let resolved_control = self.resolve_exp(control);
                let resolved_body = Box::new(self.resolve_statement(*body));
                self.break_labels.pop();
                self.switch_depth -= 1;
                let mut cases = Vec::new();
                collect_cases(&resolved_body, &mut cases);
                Statement::Switch {
                    control: resolved_control,
                    body: resolved_body,
                    label,
                    cases,
                }
            }
            Statement::Case { value, body, .. } => {
                if self.switch_depth == 0 {
                    panic!("case outside of switch");
                }
                let label = self.make_case_label();
                Statement::Case {
                    value: self.resolve_exp(value),
                    body: Box::new(self.resolve_statement(*body)),
                    label,
                }
            }
            Statement::Default { body, .. } => {
                if self.switch_depth == 0 {
                    panic!("default outside of switch");
                }
                let label = self.make_case_label();
                Statement::Default {
                    body: Box::new(self.resolve_statement(*body)),
                    label,
                }
            }
            Statement::Null => Statement::Null,
        }
    }

    fn resolve_var_decl(&mut self, vd: VarDeclaration) -> VarDeclaration {
        if vd.storage_class == Some(StorageClass::Extern) {
            let current = self.scopes.last_mut().unwrap();
            current.insert(vd.name.clone(), vd.name.clone());
            let resolved_dft = vd.decl_full_type.map(|ft| self.resolve_struct_tags_in_ft(ft));
            return VarDeclaration {
                decl_full_type: resolved_dft,
                ..vd
            };
        }
        let unique_name = self.declare_var(&vd.name);
        let init = vd.init.map(|e| self.resolve_exp(e));
        let resolved_dft = vd.decl_full_type.map(|ft| self.resolve_struct_tags_in_ft(ft));
        VarDeclaration {
            name: unique_name,
            var_type: vd.var_type,
            ptr_info: vd.ptr_info,
            array_dims: vd.array_dims,
            decl_full_type: resolved_dft,
            init,
            storage_class: vd.storage_class,
        }
    }

    fn resolve_block(&mut self, block: Block) -> Block {
        block
            .into_iter()
            .map(|item| match item {
                BlockItem::Declaration(Declaration::VarDecl(vd)) => {
                    BlockItem::Declaration(Declaration::VarDecl(self.resolve_var_decl(vd)))
                }
                BlockItem::Declaration(Declaration::FunDecl(fd)) => {
                    if let Some(existing) = self.functions.get(&fd.name) {
                        if *existing != fd.params.len() {
                            panic!("Function '{}' declared with conflicting parameter counts", fd.name);
                        }
                    }
                    self.functions.insert(fd.name.clone(), fd.params.len());
                    let resolved_rft = fd.return_full_type.map(|ft| self.resolve_struct_tags_in_ft(ft));
                    let resolved_pfts: Vec<FullType> = fd.param_full_types.into_iter()
                        .map(|ft| self.resolve_struct_tags_in_ft(ft)).collect();
                    BlockItem::Declaration(Declaration::FunDecl(FunctionDeclaration {
                        return_full_type: resolved_rft,
                        param_full_types: resolved_pfts,
                        ..fd
                    }))
                }
                BlockItem::Declaration(Declaration::StructDecl(sd)) => {
                    let unique_tag = self.declare_tag(&sd.tag);
                    // Resolve member types (for struct members that reference other structs)
                    let resolved_members: Vec<MemberDeclaration> = sd.members.into_iter().map(|m| {
                        let resolved_ft = self.resolve_struct_tags_in_ft(m.member_full_type);
                        MemberDeclaration { name: m.name, member_type: m.member_type, member_full_type: resolved_ft }
                    }).collect();
                    BlockItem::Declaration(Declaration::StructDecl(StructDeclaration { tag: unique_tag, members: resolved_members, is_union: sd.is_union }))
                }
                BlockItem::Statement(stmt) => {
                    BlockItem::Statement(self.resolve_statement(stmt))
                }
            })
            .collect()
    }

    fn resolve_function(&mut self, func: FunctionDeclaration) -> FunctionDeclaration {
        match func.body {
            None => {
                // Resolve struct tags in prototypes too
                let resolved_rft = func.return_full_type.map(|ft| self.resolve_struct_tags_in_ft(ft));
                let resolved_pfts: Vec<FullType> = func.param_full_types.into_iter()
                    .map(|ft| self.resolve_struct_tags_in_ft(ft)).collect();
                FunctionDeclaration {
                    return_full_type: resolved_rft,
                    param_full_types: resolved_pfts,
                    ..func
                }
            }
            Some(body) => {
                self.push_scope();
                self.defined_labels.clear();
                self.goto_targets.clear();
                let mut resolved_params = Vec::new();
                for (name, ptype, pi) in &func.params {
                    resolved_params.push((self.declare_var(name), *ptype, *pi));
                }
                let resolved_body = self.resolve_block(body);
                self.pop_scope();
                for target in &self.goto_targets {
                    if !self.defined_labels.contains(target) {
                        panic!("goto references undefined label: '{}'", target);
                    }
                }
                let resolved_rft = func.return_full_type.map(|ft| self.resolve_struct_tags_in_ft(ft));
                let resolved_pfts: Vec<FullType> = func.param_full_types.into_iter()
                    .map(|ft| self.resolve_struct_tags_in_ft(ft)).collect();
                FunctionDeclaration {
                    name: func.name,
                    return_type: func.return_type,
                    return_ptr_info: func.return_ptr_info, return_full_type: resolved_rft,
                    params: resolved_params,
                    body: Some(resolved_body),
                    storage_class: func.storage_class,
                    param_full_types: resolved_pfts,
                }
            }
        }
    }
}

fn collect_cases(stmt: &Statement, cases: &mut Vec<SwitchCase>) {
    match stmt {
        Statement::Case { value, body, label } => {
            let val = match value {
                Exp::Constant(c) | Exp::LongConstant(c) | Exp::UIntConstant(c) | Exp::ULongConstant(c) => *c,
                Exp::DoubleConstant(d) => *d as i64,
                _ => panic!("case value must be a constant"),
            };
            cases.push(SwitchCase { value: Some(val), label: label.clone() });
            collect_cases(body, cases);
        }
        Statement::Default { body, label } => {
            cases.push(SwitchCase { value: None, label: label.clone() });
            collect_cases(body, cases);
        }
        Statement::Block(items) => {
            for item in items {
                if let BlockItem::Statement(s) = item {
                    collect_cases(s, cases);
                }
            }
        }
        Statement::If(_, then_s, else_s) => {
            collect_cases(then_s, cases);
            if let Some(e) = else_s { collect_cases(e, cases); }
        }
        Statement::While { body, .. } => collect_cases(body, cases),
        Statement::DoWhile { body, .. } => collect_cases(body, cases),
        Statement::For { body, .. } => collect_cases(body, cases),
        Statement::Label(_, body) => collect_cases(body, cases),
        Statement::Switch { .. } => {}
        _ => {}
    }
}

pub fn resolve(program: Program) -> Program {
    let mut resolver = Resolver::new();

    for decl in &program.declarations {
        match decl {
            Declaration::FunDecl(fd) => {
                if let Some(existing) = resolver.functions.get(&fd.name) {
                    if *existing != fd.params.len() {
                        panic!("Function '{}' declared with conflicting parameter counts", fd.name);
                    }
                }
                resolver.functions.insert(fd.name.clone(), fd.params.len());
            }
            Declaration::VarDecl(vd) => {
                let global_scope = resolver.scopes.first_mut().unwrap();
                global_scope.insert(vd.name.clone(), vd.name.clone());
            }
            Declaration::StructDecl(sd) => {
                resolver.declare_tag(&sd.tag);
            }
        }
    }

    let declarations = program
        .declarations
        .into_iter()
        .map(|decl| match decl {
            Declaration::FunDecl(fd) => {
                Declaration::FunDecl(resolver.resolve_function(fd))
            }
            Declaration::VarDecl(vd) => {
                let init = vd.init.map(|e| resolver.resolve_exp(e));
                let resolved_dft = vd.decl_full_type.map(|ft| resolver.resolve_struct_tags_in_ft(ft));
                Declaration::VarDecl(VarDeclaration {
                    name: vd.name,
                    var_type: vd.var_type,
                    ptr_info: vd.ptr_info,
                    array_dims: vd.array_dims,
                    decl_full_type: resolved_dft,
                    init,
                    storage_class: vd.storage_class,
                })
            }
            Declaration::StructDecl(sd) => {
                let unique_tag = resolver.resolve_tag(&sd.tag);
                let resolved_members: Vec<MemberDeclaration> = sd.members.into_iter().map(|m| {
                    let resolved_ft = resolver.resolve_struct_tags_in_ft(m.member_full_type);
                    MemberDeclaration { name: m.name, member_type: m.member_type, member_full_type: resolved_ft }
                }).collect();
                Declaration::StructDecl(StructDeclaration { tag: unique_tag, members: resolved_members, is_union: sd.is_union })
            }
        })
        .collect();

    Program { declarations }
}
