//! Parser methods for initializers, statements, and expressions.
//! Continuation of `impl Parser` (see mod.rs).

use super::*;

impl Parser {
    pub(super) fn parse_array_init(&mut self) -> ParseResult<Exp> {
        self.expect_token(Token::OpenBrace)?;
        let mut elems = Vec::new();
        if !self.at(&Token::CloseBrace) {
            elems.push(self.parse_init_element()?);
            while self.eat(&Token::Comma) {
                if self.at(&Token::CloseBrace) {
                    break;
                } // trailing comma
                elems.push(self.parse_init_element()?);
            }
        }
        self.expect_token(Token::CloseBrace)?;
        Ok(Exp::ArrayInit(elems))
    }

    /// Parse one element of an initializer list.
    pub(super) fn parse_init_element(&mut self) -> ParseResult<Exp> {
        if matches!(self.peek(), Some(Token::Identifier(_)))
            && self.tokens.get(self.pos + 1) == Some(&Token::Colon)
        {
            let field = self.parse_identifier()?;
            self.expect_token(Token::Colon)?;
            let value = if self.at(&Token::OpenBrace) {
                self.parse_array_init()?
            } else {
                self.parse_assignment()?
            };
            return Ok(Exp::DesignatedInit(
                vec![Designator::Field(field)],
                Box::new(value),
            ));
        }
        let designators = self.parse_designators()?;
        let value = if self.at(&Token::OpenBrace) {
            self.parse_array_init()?
        } else {
            self.parse_assignment()?
        };
        if designators.is_empty() {
            Ok(value)
        } else {
            Ok(Exp::DesignatedInit(designators, Box::new(value)))
        }
    }

    /// Parse designator sequences like `.field =`, `[index] =`, or `.a.b[0] =`.
    pub(super) fn parse_designators(&mut self) -> ParseResult<Vec<Designator>> {
        let mut designators = Vec::new();
        loop {
            if self.eat(&Token::Dot) {
                designators.push(Designator::Field(self.parse_identifier()?));
            } else if self.eat(&Token::OpenBracket) {
                let index = self.parse_expression()?;
                if self.eat(&Token::Ellipsis) {
                    let end = self.parse_expression()?;
                    self.expect_token(Token::CloseBracket)?;
                    designators.push(Designator::IndexRange(Box::new(index), Box::new(end)));
                    continue;
                }
                self.expect_token(Token::CloseBracket)?;
                designators.push(Designator::Index(Box::new(index)));
            } else {
                break;
            }
        }
        if !designators.is_empty() {
            self.expect_token(Token::Assign)?;
        }
        Ok(designators)
    }

    pub(super) fn extract_array_dims(ft: &FullType) -> Option<Vec<usize>> {
        match ft {
            FullType::Array { elem, size } => {
                let mut dims = vec![*size];
                let mut inner = elem.as_ref();
                while let FullType::Array { elem: e, size: s } = inner {
                    dims.push(*s);
                    inner = e;
                }
                Some(dims)
            }
            _ => None,
        }
    }

    pub(super) fn is_type_keyword_at_pos(&self) -> bool {
        match self.peek() {
            Some(tok) => self.is_type_keyword(tok),
            None => false,
        }
    }

    /// Parse `struct tag` as a type specifier
    pub(super) fn replace_scalar_struct(ft: &FullType, tag: &str) -> FullType {
        match ft {
            FullType::Scalar(CType::Struct) => FullType::Struct(tag.to_string()),
            FullType::Pointer(inner) => {
                FullType::Pointer(Box::new(Self::replace_scalar_struct(inner, tag)))
            }
            FullType::Function {
                return_type,
                params,
                variadic,
            } => FullType::Function {
                return_type: Box::new(Self::replace_scalar_struct(return_type, tag)),
                params: params
                    .iter()
                    .map(|p| Self::replace_scalar_struct(p, tag))
                    .collect(),
                variadic: *variadic,
            },
            FullType::Array { elem, size } => FullType::Array {
                elem: Box::new(Self::replace_scalar_struct(elem, tag)),
                size: *size,
            },
            other => other.clone(),
        }
    }

    pub(super) fn validate_flexible_array_members(
        &self,
        members: &[MemberDeclaration],
        is_union: bool,
    ) -> ParseResult<()> {
        for (index, member) in members.iter().enumerate() {
            if !member.flexible_array {
                continue;
            }
            if is_union {
                return Err(self.format_error("flexible array member not allowed in union"));
            }
            if member.name.is_empty() {
                return Err(self.format_error("flexible array member must be named"));
            }
            if member.bit_width.is_some() {
                return Err(self.format_error("flexible array member cannot be a bit-field"));
            }
            if index + 1 != members.len() {
                return Err(self.format_error("flexible array member must be last"));
            }
            if members.len() == 1 {
                return Err(self.format_error("flexible array member requires a previous member"));
            }
        }
        Ok(())
    }

    pub(super) fn parse_struct_members(&mut self) -> ParseResult<Vec<MemberDeclaration>> {
        self.expect_token(Token::OpenBrace)?;
        let mut members = Vec::new();
        let mut vla_elem_sizes = Vec::new();
        while !self.at(&Token::CloseBrace) {
            if self.peek().is_none() {
                return Err(self.format_error("expected CloseBrace but found end of input"));
            }
            if self.at(&Token::KWStaticAssert) {
                self.parse_static_assert_declaration()?;
                continue;
            }
            let member_attrs = self.consume_member_attributes()?;
            let base_type = self.parse_type()?;
            let type_attrs = self.consume_member_attributes()?;
            let member_attrs = Self::merge_member_attributes(member_attrs, type_attrs);
            let base_was_enum = self.last_type_was_enum;
            let base_struct_tag = if base_type == CType::Struct {
                self.last_struct_tag.clone()
            } else {
                None
            };
            let base_typedef_full_type = self.last_typedef_full_type.clone();
            let base_typedef_vla_size = self.last_typedef_vla_size.clone();
            loop {
                self.last_typedef_full_type = base_typedef_full_type.clone();
                self.pending_flexible_array_bound = false;
                let (name, full_type, _) =
                    if base_type == CType::Struct && self.at(&Token::Semicolon) {
                        let Some(tag) = self.last_struct_tag.clone() else {
                            return Err(self.format_error("anonymous aggregate member has no type"));
                        };
                        (String::new(), FullType::Struct(tag), None)
                    } else if self.at(&Token::Colon) {
                        (String::new(), FullType::Scalar(base_type), None)
                    } else {
                        self.parse_declarator_full(base_type)?
                    };
                let flexible_array = self.pending_flexible_array_bound;
                self.pending_flexible_array_bound = false;
                // Replace Scalar(Struct) with FullType::Struct(tag)
                let mut member_full_type = if base_type == CType::Struct {
                    if let Some(ref tag) = base_struct_tag {
                        Self::replace_scalar_struct(&full_type, tag)
                    } else {
                        full_type
                    }
                } else {
                    full_type
                };
                let direct_vla_bound = self.pending_vla_bound.clone();
                let bit_width = if self.eat(&Token::Colon) {
                    let width_exp = self.parse_assignment()?;
                    let width = self
                        .eval_integer_constant_exp_with_layout(&width_exp)
                        .ok_or_else(|| {
                            self.format_error("expected integer constant bit-field width")
                        })?;
                    if width < 0 {
                        return Err(self.format_error("bit-field width must be non-negative"));
                    }
                    let width = u8::try_from(width)
                        .map_err(|_| self.format_error("bit-field width is too large"))?;
                    Some(width)
                } else {
                    None
                };
                if base_was_enum && bit_width.is_some() {
                    member_full_type = FullType::Scalar(CType::UInt);
                }
                let member_type = member_full_type.to_ctype();
                let post_attrs = self.consume_member_attributes()?;
                let member_attrs = Self::merge_member_attributes(member_attrs, post_attrs);
                if let Some(alignment) = member_attrs.alignment {
                    if let FullType::Array { elem, .. } = &member_full_type {
                        if let FullType::Struct(tag) = elem.as_ref() {
                            if let Some(def) = self.struct_defs.get_mut(tag) {
                                let align = alignment.get();
                                if align > def.alignment {
                                    def.alignment = align;
                                    let remainder = def.size % align;
                                    if remainder != 0 {
                                        def.size = def
                                            .size
                                            .checked_add(align - remainder)
                                            .ok_or_else(|| {
                                                "structure size overflows during alignment"
                                                    .to_string()
                                            })?;
                                    }
                                }
                            }
                        }
                    }
                }
                if !name.is_empty()
                    && base_typedef_vla_size.is_some()
                    && matches!(member_full_type, FullType::Array { .. })
                {
                    if let Some(base_size) = base_typedef_vla_size.clone() {
                        vla_elem_sizes.push(StructMemberVlaElemSize {
                            member: name.clone(),
                            elem_size: base_size,
                        });
                    }
                } else if !name.is_empty()
                    && matches!(
                        member_full_type,
                        FullType::Array {
                            size: VLA_STATIC_SCALE_FALLBACK,
                            ..
                        }
                    )
                {
                    if let Some(bound) = direct_vla_bound {
                        if let Some(size) =
                            Self::vla_size_expr_from_bound(bound, &member_full_type, 0)
                        {
                            vla_elem_sizes.push(StructMemberVlaElemSize {
                                member: name.clone(),
                                elem_size: size,
                            });
                        }
                    }
                }
                members.push(MemberDeclaration {
                    name,
                    member_type,
                    member_full_type,
                    bit_width,
                    flexible_array,
                    alignment: member_attrs.alignment,
                    packed: member_attrs.packed,
                });
                if self.eat(&Token::Comma) {
                    continue;
                }
                if self.at(&Token::CloseBrace) {
                    break;
                }
                self.expect_token(Token::Semicolon)?;
                break;
            }
            self.last_typedef_full_type = None;
        }
        self.pending_vla_bound = None;
        self.expect_token(Token::CloseBrace)?;
        self.pending_struct_member_vla_elem_sizes
            .push(vla_elem_sizes);
        Ok(members)
    }

    pub(super) fn parse_struct_type_specifier(&mut self) -> ParseResult<(CType, String)> {
        let is_union = self.at(&Token::KWUnion);
        if is_union {
            self.advance()?;
        } else {
            self.expect_token(Token::KWStruct)?;
        }
        let prefix_attrs = self.consume_aggregate_attributes()?;
        // Tag is optional for anonymous structs/unions
        let tag = if let Some(Token::Identifier(_)) = self.peek() {
            self.parse_identifier()?
        } else {
            // Anonymous struct/union — generate a unique tag
            let tag = format!("__anon_{}", self.pos);
            tag
        };
        // If followed by { members }, parse the struct body and record a pending definition
        if self.at(&Token::OpenBrace) {
            let members = self.parse_struct_members()?;
            let member_vla_sizes = self
                .pending_struct_member_vla_elem_sizes
                .pop()
                .unwrap_or_default();
            let suffix_attrs = self.consume_aggregate_attributes()?;
            self.validate_flexible_array_members(&members, is_union)?;
            let attrs = Self::merge_aggregate_attributes(prefix_attrs, suffix_attrs);
            let declaration = StructDeclaration {
                tag: tag.clone(),
                members,
                is_union,
                transparent_union: attrs.transparent_union,
                packed: attrs.packed,
                alignment: attrs.alignment,
                reverse_storage_order: attrs.reverse_storage_order,
            };
            self.record_struct_definition(&declaration)?;
            self.record_struct_member_vla_elem_sizes(&tag, member_vla_sizes);
            self.pending_struct_decls.push(declaration);
        }
        self.last_struct_tag = Some(tag.clone());
        Ok((CType::Struct, tag))
    }

    pub(super) fn parse_static_assert_declaration(&mut self) -> ParseResult<()> {
        self.expect_token(Token::KWStaticAssert)?;
        self.expect_token(Token::OpenParen)?;
        let condition = self.parse_assignment()?;
        let value = self
            .eval_integer_constant_exp_with_layout(&condition)
            .ok_or_else(|| self.format_error("static assertion condition must be constant"))?;
        if self.eat(&Token::Comma) {
            match self.peek() {
                Some(Token::StringLiteral(_)) => {
                    self.advance()?;
                }
                _ => return Err(self.format_error("expected string literal in static assertion")),
            }
        }
        self.expect_token(Token::CloseParen)?;
        self.expect_token(Token::Semicolon)?;
        if value == 0 {
            return Err(self.format_error("static assertion failed"));
        }
        Ok(())
    }

    pub(super) fn parse_declaration(&mut self) -> ParseResult<Declaration> {
        if self.eat(&Token::Semicolon) {
            return Ok(Declaration::TypedefDecl);
        }
        if self.at(&Token::KWStaticAssert) {
            self.parse_static_assert_declaration()?;
            return Ok(Declaration::TypedefDecl);
        }
        // Check for standalone enum declaration: enum Tag { ... };
        if self.at(&Token::KWEnum) {
            let save_pos = self.pos;
            self.advance()?; // consume 'enum'
                             // Optional tag
            if let Some(Token::Identifier(_)) = self.peek() {
                self.advance()?;
            }
            if self.at(&Token::OpenBrace) {
                self.pos = save_pos;
                // Parse through parse_specifiers which handles enum body
                let (_sc, _base_type) = self.parse_specifiers()?;
                self.last_typedef_full_type = None;
                if self.at(&Token::Semicolon) {
                    self.advance()?;
                    return Ok(Declaration::TypedefDecl); // no-op, constants already registered
                }
                // Not a standalone enum — has a variable name after. Put back and re-parse.
                self.pos = save_pos;
            } else {
                self.pos = save_pos;
            }
        }
        // Check for standalone struct/union declaration: struct/union tag { members };
        if self.at(&Token::KWStruct) || self.at(&Token::KWUnion) {
            let is_union = self.at(&Token::KWUnion);
            let save_pos = self.pos;
            self.advance()?; // consume 'struct' or 'union'
            let prefix_attrs = self.consume_aggregate_attributes()?;
            if let Some(Token::Identifier(_)) = self.peek() {
                let tag = self.parse_identifier()?;
                if self.at(&Token::OpenBrace) {
                    let members = self.parse_struct_members()?;
                    let member_vla_sizes = self
                        .pending_struct_member_vla_elem_sizes
                        .pop()
                        .unwrap_or_default();
                    let suffix_attrs = self.consume_aggregate_attributes()?;
                    self.validate_flexible_array_members(&members, is_union)?;
                    if self.at(&Token::Semicolon) {
                        self.advance()?;
                        let attrs = Self::merge_aggregate_attributes(prefix_attrs, suffix_attrs);
                        let declaration = StructDeclaration {
                            tag: tag.clone(),
                            members,
                            is_union,
                            transparent_union: attrs.transparent_union,
                            packed: attrs.packed,
                            alignment: attrs.alignment,
                            reverse_storage_order: attrs.reverse_storage_order,
                        };
                        self.record_struct_definition(&declaration)?;
                        self.record_struct_member_vla_elem_sizes(&tag, member_vla_sizes);
                        return Ok(Declaration::StructDecl(declaration));
                    }
                } else if self.at(&Token::Semicolon) {
                    self.advance()?;
                    return Ok(Declaration::StructDecl(StructDeclaration {
                        tag,
                        members: vec![],
                        is_union,
                        transparent_union: prefix_attrs.transparent_union,
                        packed: prefix_attrs.packed,
                        alignment: prefix_attrs.alignment,
                        reverse_storage_order: prefix_attrs.reverse_storage_order,
                    }));
                }
            }
            // Not a standalone decl — put back and let parse_specifiers handle it
            self.pos = save_pos;
        }
        if let Some(Token::Identifier(name)) = self.peek().cloned() {
            if !self.is_type_keyword(&Token::Identifier(name.clone()))
                && self.pos + 1 < self.tokens.len()
                && self.tokens[self.pos + 1] == Token::OpenParen
            {
                let name = self.parse_identifier()?;
                self.expect_token(Token::OpenParen)?;
                let (
                    params,
                    param_full_types,
                    deprecated_params,
                    variadic,
                    zero_fixed_variadic,
                    old_style,
                    param_vla_bounds,
                ) = self.parse_param_list()?;
                self.expect_token(Token::CloseParen)?;
                let func_info = FunctionDeclaratorInfo {
                    params,
                    param_full_types,
                    deprecated_params,
                    variadic,
                    zero_fixed_variadic,
                    old_style,
                    param_vla_bounds,
                };
                let func_info = if self.at(&Token::Semicolon) || self.at(&Token::OpenBrace) {
                    func_info
                } else {
                    self.parse_old_style_param_declarations(func_info)?
                };
                let return_ft = FullType::Scalar(CType::Int);
                self.add_value_type(
                    name.clone(),
                    Self::function_full_type(return_ft.clone(), &func_info),
                )?;
                let param_value_types =
                    Self::param_value_types(&func_info.params, &func_info.param_full_types);
                let body = if self.at(&Token::OpenBrace) {
                    Some(self.parse_function_body_preserving_type_decls(
                        &name,
                        &param_value_types,
                        &func_info.param_vla_bounds,
                    )?)
                } else {
                    self.expect_token(Token::Semicolon)?;
                    None
                };
                return Ok(Declaration::FunDecl(FunctionDeclaration {
                    name,
                    return_type: CType::Int,
                    return_ptr_info: None,
                    return_full_type: Some(return_ft),
                    params: func_info.params,
                    body,
                    storage_class: None,
                    param_full_types: func_info.param_full_types,
                    param_vla_bounds: func_info.param_vla_bounds,
                    deprecated_params: func_info.deprecated_params,
                    variadic: func_info.variadic,
                    zero_fixed_variadic: func_info.zero_fixed_variadic,
                    old_style: func_info.old_style,
                    noreturn: false,
                    no_instrument_function: false,
                    is_inline: false,
                }));
            }
        }
        let (mut sc, base_type) = self.parse_specifiers()?;
        let is_auto_type = std::mem::take(&mut self.pending_auto_type);
        let spec_noreturn = std::mem::take(&mut self.pending_noreturn);
        let spec_no_instrument = std::mem::take(&mut self.pending_no_instrument_function);
        let spec_inline = std::mem::take(&mut self.pending_inline);
        let decl_alignment = self.pending_alignment.take();
        sc = self.consume_post_type_storage_class(sc)?;
        if self.at(&Token::Semicolon) && (base_type == CType::Struct || self.last_type_was_enum) {
            self.advance()?;
            return Ok(Declaration::TypedefDecl);
        }
        // Save struct tag before declarator parsing (params may overwrite last_struct_tag)
        let saved_struct_tag = if base_type == CType::Struct {
            self.last_struct_tag.clone()
        } else {
            None
        };
        let base_typedef_full_type = self.last_typedef_full_type.clone();

        let pre_attrs = self.consume_declaration_attributes()?;
        let decl_tree = self.parse_declarator_tree()?;
        let post_attrs = self.consume_declaration_attributes()?;
        let first_alignment = [decl_alignment, pre_attrs.alignment, post_attrs.alignment]
            .into_iter()
            .flatten()
            .max();
        let first_noreturn = spec_noreturn || pre_attrs.noreturn || post_attrs.noreturn;
        let first_no_instrument =
            spec_no_instrument || std::mem::take(&mut self.pending_no_instrument_function);
        let decl_transparent_union = std::mem::take(&mut self.pending_transparent_union);
        let td_ft = base_typedef_full_type;
        self.last_typedef_full_type = None;
        let (name, full_type, decl_params) =
            Self::process_declarator(&decl_tree, base_type, td_ft.as_ref());
        let full_type = self
            .apply_vector_size_attr(full_type, post_attrs.vector_size.or(pre_attrs.vector_size));

        // Replace Scalar(Struct) with FullType::Struct(tag) if applicable
        let full_type = if base_type == CType::Struct {
            if let Some(ref tag) = saved_struct_tag {
                if decl_transparent_union {
                    self.mark_pending_transparent_union(tag);
                }
                Self::replace_scalar_struct(&full_type, tag)
            } else {
                full_type
            }
        } else {
            full_type
        };

        if is_auto_type {
            if sc.as_ref().is_some_and(StorageClass::is_typedef)
                || decl_params.is_some()
                || full_type != FullType::Scalar(CType::Void)
            {
                return Err(self.format_error("__auto_type requires a single object declarator"));
            }
            let decl = self.make_auto_var_decl(name, sc, first_alignment)?;
            self.expect_token(Token::Semicolon)?;
            self.add_value_type(decl.name.clone(), self.var_decl_full_type(&decl)?)?;
            return Ok(Declaration::VarDecl(decl));
        }

        // Handle typedef declarations
        if sc.as_ref().is_some_and(StorageClass::is_typedef) {
            let full_type = decl_params
                .as_ref()
                .map(|info| Self::function_full_type(full_type.clone(), info))
                .unwrap_or(full_type);
            let vla_size = self.typedef_vla_size_expr(&full_type);
            self.add_typedef(
                name,
                TypedefInfo {
                    base_type: full_type.to_ctype(),
                    full_type: full_type.clone(),
                    struct_tag: saved_struct_tag.clone(),
                    is_enum: self.last_type_was_enum,
                    vla_size,
                    alignment: first_alignment,
                },
            )?;
            while self.eat(&Token::Comma) {
                self.last_typedef_full_type = td_ft.clone();
                let decl_tree = self.parse_declarator_tree()?;
                let (name2, full_type2, decl_params2) =
                    Self::process_declarator(&decl_tree, base_type, td_ft.as_ref());
                let full_type2 = self.apply_vector_size_attr(full_type2, post_attrs.vector_size);
                let full_type2 = if base_type == CType::Struct {
                    if let Some(ref tag) = saved_struct_tag {
                        Self::replace_scalar_struct(&full_type2, tag)
                    } else {
                        full_type2
                    }
                } else {
                    full_type2
                };
                let full_type2 = decl_params2
                    .as_ref()
                    .map(|info| Self::function_full_type(full_type2.clone(), info))
                    .unwrap_or(full_type2);
                let vla_size = self.typedef_vla_size_expr(&full_type2);
                self.add_typedef(
                    name2,
                    TypedefInfo {
                        base_type: full_type2.to_ctype(),
                        full_type: full_type2,
                        struct_tag: saved_struct_tag.clone(),
                        is_enum: self.last_type_was_enum,
                        vla_size,
                        alignment: first_alignment,
                    },
                )?;
            }
            self.expect_token(Token::Semicolon)?;
            return Ok(Declaration::TypedefDecl);
        }

        // Derive scalar metadata from the canonical FullType.
        let ctype = full_type.to_ctype();
        let pi = match &full_type {
            FullType::Pointer(inner) => {
                let (base, depth) = ptr_info_from_full(inner);
                Some((base, depth))
            }
            _ => None,
        };
        // Is it a function?
        if let Some(func_info) = decl_params {
            let func_info = if self.at(&Token::Semicolon)
                || self.at(&Token::OpenBrace)
                || self.at(&Token::Comma)
            {
                func_info
            } else {
                self.parse_old_style_param_declarations(func_info)?
            };
            self.add_value_type(
                name.clone(),
                Self::function_full_type(full_type.clone(), &func_info),
            )?;
            let param_value_types =
                Self::param_value_types(&func_info.params, &func_info.param_full_types);
            let body = if self.at(&Token::OpenBrace) {
                Some(self.parse_function_body_preserving_type_decls(
                    &name,
                    &param_value_types,
                    &func_info.param_vla_bounds,
                )?)
            } else {
                if self.eat(&Token::Comma) {
                    let mut extra = Vec::new();
                    loop {
                        let decl_tree = self.parse_declarator_tree()?;
                        let (name2, full_type2, decl_params2) =
                            Self::process_declarator(&decl_tree, base_type, td_ft.as_ref());
                        let full_type2 = if base_type == CType::Struct {
                            if let Some(ref tag) = saved_struct_tag {
                                Self::replace_scalar_struct(&full_type2, tag)
                            } else {
                                full_type2
                            }
                        } else {
                            full_type2
                        };
                        let ctype2 = full_type2.to_ctype();
                        let pi2 = match &full_type2 {
                            FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                            _ => None,
                        };
                        if let Some(func_info2) = decl_params2 {
                            self.add_value_type(
                                name2.clone(),
                                Self::function_full_type(full_type2.clone(), &func_info2),
                            )?;
                            if let Some(alignment) = first_alignment {
                                self.function_alignments
                                    .insert(name2.clone(), alignment.get());
                            }
                            extra.push(Declaration::FunDecl(FunctionDeclaration {
                                name: name2,
                                return_type: ctype2,
                                return_ptr_info: pi2,
                                return_full_type: Some(full_type2),
                                params: func_info2.params,
                                body: None,
                                storage_class: sc.clone(),
                                param_full_types: func_info2.param_full_types,
                                param_vla_bounds: func_info2.param_vla_bounds,
                                deprecated_params: func_info2.deprecated_params,
                                variadic: func_info2.variadic,
                                zero_fixed_variadic: func_info2.zero_fixed_variadic,
                                old_style: func_info2.old_style,
                                noreturn: first_noreturn,
                                no_instrument_function: first_no_instrument,
                                is_inline: spec_inline,
                            }));
                        } else {
                            let decl = self.make_var_decl(
                                name2,
                                &full_type2,
                                ctype2,
                                pi2,
                                sc.clone(),
                                first_alignment,
                            )?;
                            self.add_value_type(
                                decl.name.clone(),
                                self.var_decl_full_type(&decl)?,
                            )?;
                            extra.push(Declaration::VarDecl(decl));
                        }
                        if !self.eat(&Token::Comma) {
                            break;
                        }
                    }
                    self.expect_token(Token::Semicolon)?;
                    self.pending_declarations.extend(extra);
                } else {
                    self.expect_token(Token::Semicolon)?;
                }
                None
            };
            if let Some(alignment) = first_alignment {
                self.function_alignments
                    .insert(name.clone(), alignment.get());
            }
            return Ok(Declaration::FunDecl(FunctionDeclaration {
                name,
                return_type: ctype,
                return_ptr_info: pi,
                return_full_type: Some(full_type.clone()),
                params: func_info.params,
                body,
                storage_class: sc,
                param_full_types: func_info.param_full_types,
                param_vla_bounds: func_info.param_vla_bounds,
                deprecated_params: func_info.deprecated_params,
                variadic: func_info.variadic,
                zero_fixed_variadic: func_info.zero_fixed_variadic,
                old_style: func_info.old_style,
                noreturn: first_noreturn,
                no_instrument_function: first_no_instrument,
                is_inline: spec_inline,
            }));
        }

        // Check for function (in case declarator didn't catch params)
        if self.at(&Token::OpenParen) {
            self.expect_token(Token::OpenParen)?;
            let (
                params,
                param_fts,
                deprecated_params,
                variadic,
                zero_fixed_variadic,
                old_style,
                param_vla_bounds,
            ) = self.parse_param_list()?;
            self.expect_token(Token::CloseParen)?;
            let func_info = FunctionDeclaratorInfo {
                params,
                param_full_types: param_fts,
                deprecated_params,
                variadic,
                zero_fixed_variadic,
                old_style,
                param_vla_bounds,
            };
            let func_info = if self.at(&Token::Semicolon)
                || self.at(&Token::OpenBrace)
                || self.at(&Token::Comma)
            {
                func_info
            } else {
                self.parse_old_style_param_declarations(func_info)?
            };
            self.add_value_type(
                name.clone(),
                Self::function_full_type(full_type.clone(), &func_info),
            )?;
            let param_value_types =
                Self::param_value_types(&func_info.params, &func_info.param_full_types);
            let body = if self.at(&Token::OpenBrace) {
                Some(self.parse_function_body_preserving_type_decls(
                    &name,
                    &param_value_types,
                    &func_info.param_vla_bounds,
                )?)
            } else {
                if self.eat(&Token::Comma) {
                    let mut extra = Vec::new();
                    loop {
                        let decl_tree = self.parse_declarator_tree()?;
                        let (name2, full_type2, decl_params2) =
                            Self::process_declarator(&decl_tree, base_type, td_ft.as_ref());
                        let full_type2 = if base_type == CType::Struct {
                            if let Some(ref tag) = saved_struct_tag {
                                Self::replace_scalar_struct(&full_type2, tag)
                            } else {
                                full_type2
                            }
                        } else {
                            full_type2
                        };
                        let ctype2 = full_type2.to_ctype();
                        let pi2 = match &full_type2 {
                            FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                            _ => None,
                        };
                        if let Some(func_info2) = decl_params2 {
                            self.add_value_type(
                                name2.clone(),
                                Self::function_full_type(full_type2.clone(), &func_info2),
                            )?;
                            extra.push(Declaration::FunDecl(FunctionDeclaration {
                                name: name2,
                                return_type: ctype2,
                                return_ptr_info: pi2,
                                return_full_type: Some(full_type2),
                                params: func_info2.params,
                                body: None,
                                storage_class: sc.clone(),
                                param_full_types: func_info2.param_full_types,
                                param_vla_bounds: func_info2.param_vla_bounds,
                                deprecated_params: func_info2.deprecated_params,
                                variadic: func_info2.variadic,
                                zero_fixed_variadic: func_info2.zero_fixed_variadic,
                                old_style: func_info2.old_style,
                                noreturn: first_noreturn,
                                no_instrument_function: first_no_instrument,
                                is_inline: spec_inline,
                            }));
                        } else {
                            let decl = self.make_var_decl(
                                name2,
                                &full_type2,
                                ctype2,
                                pi2,
                                sc.clone(),
                                first_alignment,
                            )?;
                            self.add_value_type(
                                decl.name.clone(),
                                self.var_decl_full_type(&decl)?,
                            )?;
                            extra.push(Declaration::VarDecl(decl));
                        }
                        if !self.eat(&Token::Comma) {
                            break;
                        }
                    }
                    self.expect_token(Token::Semicolon)?;
                    self.pending_declarations.extend(extra);
                } else {
                    self.expect_token(Token::Semicolon)?;
                }
                None
            };
            if let Some(alignment) = first_alignment {
                self.function_alignments
                    .insert(name.clone(), alignment.get());
            }
            return Ok(Declaration::FunDecl(FunctionDeclaration {
                name,
                return_type: ctype,
                return_ptr_info: pi,
                return_full_type: Some(full_type.clone()),
                params: func_info.params,
                body,
                storage_class: sc,
                param_full_types: func_info.param_full_types,
                param_vla_bounds: func_info.param_vla_bounds,
                deprecated_params: func_info.deprecated_params,
                variadic: func_info.variadic,
                zero_fixed_variadic: func_info.zero_fixed_variadic,
                old_style: func_info.old_style,
                noreturn: first_noreturn,
                no_instrument_function: first_no_instrument,
                is_inline: spec_inline,
            }));
        }

        let first = self.make_var_decl(name, &full_type, ctype, pi, sc.clone(), first_alignment)?;
        self.add_value_type(first.name.clone(), self.var_decl_full_type(&first)?)?;
        // Check for multiple declarators
        if self.eat(&Token::Comma) {
            let mut extra = Vec::new();
            loop {
                let decl_tree = self.parse_declarator_tree()?;
                let (name2, full_type2, decl_params2) =
                    Self::process_declarator(&decl_tree, base_type, td_ft.as_ref());
                let post_alignment = self.consume_alignment_specifiers()?;
                let declarator_alignment = match (decl_alignment, post_alignment) {
                    (Some(current), Some(post)) => Some(current.max(post)),
                    (Some(current), None) => Some(current),
                    (None, Some(post)) => Some(post),
                    (None, None) => None,
                };
                let full_type2 = if base_type == CType::Struct {
                    if let Some(ref tag) = saved_struct_tag {
                        Self::replace_scalar_struct(&full_type2, tag)
                    } else {
                        full_type2
                    }
                } else {
                    full_type2
                };
                let ctype2 = full_type2.to_ctype();
                let pi2 = match &full_type2 {
                    FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                    _ => None,
                };
                if let Some(func_info2) = decl_params2 {
                    self.add_value_type(
                        name2.clone(),
                        Self::function_full_type(full_type2.clone(), &func_info2),
                    )?;
                    extra.push(Declaration::FunDecl(FunctionDeclaration {
                        name: name2,
                        return_type: ctype2,
                        return_ptr_info: pi2,
                        return_full_type: Some(full_type2),
                        params: func_info2.params,
                        body: None,
                        storage_class: sc.clone(),
                        param_full_types: func_info2.param_full_types,
                        param_vla_bounds: func_info2.param_vla_bounds,
                        deprecated_params: func_info2.deprecated_params,
                        variadic: func_info2.variadic,
                        zero_fixed_variadic: func_info2.zero_fixed_variadic,
                        old_style: func_info2.old_style,
                        noreturn: first_noreturn,
                        no_instrument_function: first_no_instrument,
                        is_inline: spec_inline,
                    }));
                } else {
                    let decl = self.make_var_decl(
                        name2,
                        &full_type2,
                        ctype2,
                        pi2,
                        sc.clone(),
                        declarator_alignment,
                    )?;
                    self.add_value_type(decl.name.clone(), self.var_decl_full_type(&decl)?)?;
                    extra.push(Declaration::VarDecl(decl));
                }
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            self.expect_token(Token::Semicolon)?;
            self.pending_declarations.extend(extra);
        } else {
            self.expect_token(Token::Semicolon)?;
        }
        Ok(Declaration::VarDecl(first))
    }

    pub(super) fn parse_var_declaration(&mut self) -> ParseResult<VarDeclaration> {
        let (sc, base_type) = self.parse_specifiers()?;
        let is_auto_type = std::mem::take(&mut self.pending_auto_type);
        let _spec_noreturn = std::mem::take(&mut self.pending_noreturn);
        let _spec_inline = std::mem::take(&mut self.pending_inline);
        let decl_alignment = self.pending_alignment.take();
        let (name, full_type, _) = self.parse_declarator_full(base_type)?;
        let post_alignment = self.consume_alignment_specifiers()?;
        let decl_alignment = match (decl_alignment, post_alignment) {
            (Some(current), Some(post)) => Some(current.max(post)),
            (Some(current), None) => Some(current),
            (None, Some(post)) => Some(post),
            (None, None) => None,
        };
        let full_type = if base_type == CType::Struct {
            if let Some(ref tag) = self.last_struct_tag {
                Self::replace_scalar_struct(&full_type, tag)
            } else {
                full_type
            }
        } else {
            full_type
        };
        if is_auto_type {
            if full_type != FullType::Scalar(CType::Void) {
                return Err(self.format_error("__auto_type requires a single object declarator"));
            }
            let decl = self.make_auto_var_decl(name, sc, decl_alignment)?;
            self.expect_token(Token::Semicolon)?;
            self.add_value_type(decl.name.clone(), self.var_decl_full_type(&decl)?)?;
            return Ok(decl);
        }
        let ctype = full_type.to_ctype();
        let pi = match &full_type {
            FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
            _ => None,
        };
        let init = if self.eat(&Token::Assign) {
            if self.at(&Token::OpenBrace) {
                Some(self.parse_array_init()?)
            } else {
                Some(self.parse_expression()?)
            }
        } else {
            None
        };
        let full_type = self.infer_unsized_array_type(full_type, init.as_ref())?;
        let array_dims = Self::extract_array_dims(&full_type);
        if ctype == CType::Void && array_dims.is_none() {
            return Err(self.format_error("cannot declare variable with void type"));
        }
        self.expect_token(Token::Semicolon)?;
        self.add_value_type(name.clone(), full_type.clone())?;
        Ok(VarDeclaration {
            name,
            var_type: if array_dims.is_some() {
                let mut t = &full_type;
                while let FullType::Array { elem, .. } = t {
                    t = elem;
                }
                t.to_ctype()
            } else {
                ctype
            },
            ptr_info: pi,
            array_dims,
            decl_full_type: Some(full_type.clone()),
            dynamic_size: self
                .dynamic_size_expr_for_decl_type(&full_type)
                .map(Box::new),
            init,
            storage_class: sc,
            alignment: decl_alignment,
            alias: self.pending_alias.take(),
        })
    }

    #[allow(clippy::type_complexity)]
    pub(super) fn parse_param_list(
        &mut self,
    ) -> ParseResult<(
        Vec<ParamDecl>,
        Vec<FullType>,
        Vec<DeprecatedParam>,
        bool,
        bool,
        bool,
        Vec<Exp>,
    )> {
        // "void" or empty or "int x, long y, ..."
        if self.at(&Token::KWVoid)
            && self.pos + 1 < self.tokens.len()
            && self.tokens[self.pos + 1] == Token::CloseParen
        {
            self.advance()?;
            return Ok((
                Vec::new(),
                Vec::new(),
                Vec::new(),
                false,
                false,
                false,
                Vec::new(),
            ));
        }
        if self.at(&Token::CloseParen) {
            return Ok((
                Vec::new(),
                Vec::new(),
                Vec::new(),
                true,
                false,
                false,
                Vec::new(),
            ));
        }
        if self.eat(&Token::Ellipsis) {
            return Ok((
                Vec::new(),
                Vec::new(),
                Vec::new(),
                true,
                true,
                false,
                Vec::new(),
            ));
        }
        if let Some(Token::Identifier(name)) = self.peek().cloned() {
            if !self.is_type_keyword(&Token::Identifier(name.clone())) {
                let mut params = Vec::new();
                let mut param_fts = Vec::new();
                loop {
                    let name = self.parse_identifier()?;
                    params.push((name, CType::Int, None));
                    param_fts.push(FullType::Scalar(CType::Int));
                    if !self.eat(&Token::Comma) {
                        break;
                    }
                }
                return Ok((
                    params,
                    param_fts,
                    Vec::new(),
                    false,
                    false,
                    true,
                    Vec::new(),
                ));
            }
        }
        let mut params = Vec::new();
        let mut param_fts = Vec::new();
        let mut deprecated_params = Vec::new();
        let mut param_vla_bounds = Vec::new();
        let parse_one_param = |s: &mut Self,
                               fts: &mut Vec<FullType>,
                               vla_bounds: &mut Vec<Exp>|
         -> ParseResult<(ParamDecl, Option<DeprecatedParam>)> {
            s.pending_deprecated_param = None;
            s.param_parse_depth += 1;
            let base = s.parse_type()?;
            // Use abstract declarator parsing (name optional) for params
            let tree = s.parse_declarator_tree_inner(true)?;
            s.consume_declarator_qualifiers()?;
            s.param_parse_depth -= 1;
            let deprecated_message = s.pending_deprecated_param.take();
            let td_ft = s.last_typedef_full_type.take();
            let (name, full_type, decl_params) =
                Self::process_declarator(&tree, base, td_ft.as_ref());
            let full_type = decl_params
                .as_ref()
                .map(|info| Self::function_full_type(full_type.clone(), info))
                .unwrap_or(full_type);
            if let Some(bound) = s.pending_vla_bound.take() {
                vla_bounds.push(bound);
            }
            // Generate a dummy name for unnamed params
            let name = if name.is_empty() {
                format!("__unnamed_{}", s.pos)
            } else {
                name
            };
            let deprecated = deprecated_message.map(|message| DeprecatedParam {
                name: name.clone(),
                message,
            });
            // Replace Scalar(Struct) with FullType::Struct(tag)
            let full_type = if base == CType::Struct {
                if let Some(ref tag) = s.last_struct_tag {
                    Self::replace_scalar_struct(&full_type, tag)
                } else {
                    full_type
                }
            } else {
                full_type
            };
            // Array parameters decay to pointers (first dimension dropped)
            let ft = match full_type {
                FullType::Array { elem, .. } => FullType::Pointer(elem),
                FullType::Function { .. } => FullType::Pointer(Box::new(full_type)),
                other => other,
            };
            fts.push(ft.clone());
            let t = ft.to_ctype();
            let pi = match &ft {
                FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                _ => None,
            };
            Ok(((name, t, pi), deprecated))
        };
        let (param, deprecated) = parse_one_param(self, &mut param_fts, &mut param_vla_bounds)?;
        if let Some(deprecated) = deprecated {
            deprecated_params.push(deprecated);
        }
        params.push(param);
        let mut variadic = false;
        while self.eat(&Token::Comma) {
            // Check for ... (variadic)
            if self.eat(&Token::Ellipsis) {
                variadic = true;
                break;
            }
            let (param, deprecated) = parse_one_param(self, &mut param_fts, &mut param_vla_bounds)?;
            if let Some(deprecated) = deprecated {
                deprecated_params.push(deprecated);
            }
            params.push(param);
        }
        Ok((
            params,
            param_fts,
            deprecated_params,
            variadic,
            false,
            false,
            param_vla_bounds,
        ))
    }

    pub(super) fn parse_old_style_param_declarations(
        &mut self,
        mut info: FunctionDeclaratorInfo,
    ) -> ParseResult<FunctionDeclaratorInfo> {
        info.old_style = true;
        info.zero_fixed_variadic = false;
        while !self.at(&Token::OpenBrace) {
            let (_sc, base_type) = self.parse_specifiers()?;
            let td_ft = self.last_typedef_full_type.take();
            loop {
                let decl_tree = self.parse_declarator_tree()?;
                let (name, full_type, decl_params) =
                    Self::process_declarator(&decl_tree, base_type, td_ft.as_ref());
                if decl_params.is_some() {
                    return Err(
                        self.format_error("old-style function parameter cannot be a function")
                    );
                }
                let full_type = if base_type == CType::Struct {
                    if let Some(ref tag) = self.last_struct_tag {
                        Self::replace_scalar_struct(&full_type, tag)
                    } else {
                        full_type
                    }
                } else {
                    full_type
                };
                let Some(index) = info.params.iter().position(|(param, _, _)| param == &name)
                else {
                    return Err(self.format_error(&format!(
                        "old-style parameter declaration for unknown parameter '{}'",
                        name
                    )));
                };
                let ft = match full_type {
                    FullType::Array { elem, .. } => FullType::Pointer(elem),
                    FullType::Function { .. } => FullType::Pointer(Box::new(full_type)),
                    other => other,
                };
                let ty = ft.to_ctype();
                let pi = match &ft {
                    FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                    _ => None,
                };
                info.params[index] = (name, ty, pi);
                info.param_full_types[index] = ft;
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            self.expect_token(Token::Semicolon)?;
        }
        Ok(info)
    }

    pub(super) fn parse_identifier(&mut self) -> ParseResult<String> {
        match self.advance()? {
            Token::Identifier(name) => Ok(name),
            other => {
                self.pos -= 1;
                Err(self.format_error(&format!("expected identifier, found {:?}", other)))
            }
        }
    }

    // --------------------------------------------------------
    // Blocks and block items
    // --------------------------------------------------------

    pub(super) fn parse_block(&mut self) -> ParseResult<Block> {
        self.parse_block_with_values(&[])
    }

    pub(super) fn parse_scoped_block_items(&mut self) -> ParseResult<Block> {
        let mut items = Vec::new();
        while !self.at(&Token::CloseBrace) {
            if self.peek().is_none() {
                return Err(self.format_error("expected CloseBrace but found end of input"));
            }
            let item = if self.is_block_label_start() {
                BlockItem::Statement(self.parse_block_label_statement()?)
            } else {
                self.parse_block_item()?
            };
            items.append(&mut self.pending_pre_block_items);
            // Emit any pending struct/union definitions from type specifier parsing
            for sd in self.pending_struct_decls.drain(..) {
                items.push(BlockItem::Declaration(Declaration::StructDecl(sd)));
            }
            items.push(item);
            // Emit extra items from multi-declarator parsing
            items.append(&mut self.pending_block_items);
        }
        Ok(items)
    }

    pub(super) fn is_block_label_start(&self) -> bool {
        matches!(self.peek(), Some(Token::KWCase | Token::KWDefault))
    }

    pub(super) fn parse_block_label_statement(&mut self) -> ParseResult<Statement> {
        match self.peek() {
            Some(Token::KWCase) => {
                self.advance()?;
                let value = self.parse_expression()?;
                let end_value = if self.eat(&Token::Ellipsis) {
                    Some(self.parse_expression()?)
                } else {
                    None
                };
                self.expect_token(Token::Colon)?;
                Ok(Statement::Case {
                    value,
                    end_value,
                    body: Box::new(Statement::Null),
                    label: String::new(),
                })
            }
            Some(Token::KWDefault) => {
                self.advance()?;
                self.expect_token(Token::Colon)?;
                Ok(Statement::Default {
                    body: Box::new(Statement::Null),
                    label: String::new(),
                })
            }
            other => Err(self.format_error(&format!("expected block label, found {:?}", other))),
        }
    }

    pub(super) fn parse_block_with_values(
        &mut self,
        initial_values: &[(String, FullType)],
    ) -> ParseResult<Block> {
        self.expect_token(Token::OpenBrace)?;
        self.push_typedef_scope();
        for (name, full_type) in initial_values {
            self.add_value_type(name.clone(), full_type.clone())?;
        }
        let items = match self.parse_scoped_block_items() {
            Ok(items) => items,
            Err(err) => {
                self.pop_typedef_scope();
                return Err(err);
            }
        };
        self.expect_token(Token::CloseBrace)?;
        self.pop_typedef_scope();
        Ok(items)
    }

    pub(super) fn parse_function_body_with_values(
        &mut self,
        function_name: &str,
        initial_values: &[(String, FullType)],
        param_vla_bounds: &[Exp],
    ) -> ParseResult<Block> {
        let previous = self
            .current_function_name
            .replace(function_name.to_string());
        self.expect_token(Token::OpenBrace)?;
        self.push_typedef_scope();
        let mut captured_bounds = Vec::new();
        for bound in param_vla_bounds {
            let name = format!("__rnqcc_param_vla_bound_{}", self.vla_bound_counter);
            self.vla_bound_counter += 1;
            self.add_value_type(name.clone(), FullType::Scalar(CType::Long))?;
            captured_bounds.push((
                Exp::Var(name.clone()),
                BlockItem::Declaration(Declaration::VarDecl(VarDeclaration {
                    name,
                    var_type: CType::Long,
                    ptr_info: None,
                    array_dims: None,
                    decl_full_type: Some(FullType::Scalar(CType::Long)),
                    dynamic_size: None,
                    init: Some(bound.clone()),
                    storage_class: None,
                    alignment: None,
                    alias: None,
                })),
            ));
        }
        let mut bounds = captured_bounds.iter().map(|(bound, _)| bound);
        for (name, full_type) in initial_values {
            self.add_value_type(name.clone(), full_type.clone())?;
            if let FullType::Pointer(pointee) = full_type {
                if let FullType::Array { elem, size } = pointee.as_ref() {
                    if *size == VLA_STATIC_SCALE_FALLBACK {
                        if let Some(bound) = bounds.next() {
                            self.add_value_vla_elem_size(
                                name.clone(),
                                Exp::Binary(
                                    BinaryOp::Mul,
                                    Box::new(bound.clone()),
                                    Box::new(Exp::SizeOfType(
                                        elem.to_ctype(),
                                        elem.as_ref().clone(),
                                    )),
                                ),
                            )?;
                        }
                    }
                }
            }
        }
        let result = match self.parse_scoped_block_items() {
            Ok(mut block) => {
                for (_, item) in captured_bounds.iter().rev() {
                    block.insert(0, item.clone());
                }
                match self.expect_token(Token::CloseBrace) {
                    Ok(()) => Ok(block),
                    Err(err) => Err(err),
                }
            }
            Err(err) => Err(err),
        };
        self.pop_typedef_scope();
        self.current_function_name = previous;
        result
    }

    pub(super) fn parse_function_body_preserving_type_decls(
        &mut self,
        function_name: &str,
        initial_values: &[(String, FullType)],
        param_vla_bounds: &[Exp],
    ) -> ParseResult<Block> {
        let mut pending_type_decls = std::mem::take(&mut self.pending_struct_decls);
        let body =
            self.parse_function_body_with_values(function_name, initial_values, param_vla_bounds)?;
        pending_type_decls.append(&mut self.pending_struct_decls);
        self.pending_struct_decls = pending_type_decls;
        Ok(body)
    }

    pub(super) fn parse_statement_expression(&mut self) -> ParseResult<Exp> {
        self.expect_token(Token::OpenParen)?;
        self.expect_token(Token::OpenBrace)?;
        self.push_typedef_scope();
        let mut block = match self.parse_scoped_block_items() {
            Ok(items) => items,
            Err(err) => {
                self.pop_typedef_scope();
                return Err(err);
            }
        };
        self.expect_token(Token::CloseBrace)?;
        let (result, result_type) = match block.pop() {
            Some(BlockItem::Statement(Statement::Expression(exp))) => {
                let full_type = self.typeof_expression(&exp)?;
                (Some(Box::new(exp)), Some(full_type))
            }
            Some(item) => {
                block.push(item);
                (None, None)
            }
            None => (None, None),
        };
        self.pop_typedef_scope();
        self.expect_token(Token::CloseParen)?;
        Ok(Exp::StatementExpr(block, result, result_type))
    }

    pub(super) fn is_declaration_start(&self) -> bool {
        match self.peek() {
            Some(Token::KWInt)
            | Some(Token::KWLong)
            | Some(Token::KWUnsigned)
            | Some(Token::KWSigned)
            | Some(Token::KWDouble)
            | Some(Token::KWFloat)
            | Some(Token::KWVoid)
            | Some(Token::KWChar)
            | Some(Token::KWStruct)
            | Some(Token::KWUnion)
            | Some(Token::KWAlignOf)
            | Some(Token::KWEnum)
            | Some(Token::KWStatic)
            | Some(Token::KWExtern)
            | Some(Token::KWTypedef)
            | Some(Token::KWConst)
            | Some(Token::KWVolatile)
            | Some(Token::KWAtomic)
            | Some(Token::KWThreadLocal)
            | Some(Token::KWStaticAssert)
            | Some(Token::KWInline)
            | Some(Token::KWRegister)
            | Some(Token::KWAuto)
            | Some(Token::KWRestrict)
            | Some(Token::KWBool)
            | Some(Token::KWShort)
            | Some(Token::KWTypeOf)
            | Some(Token::KWTypeOfUnqual)
            | Some(Token::KWAutoType)
            | Some(Token::AttributeAligned(_))
            | Some(Token::AttributeAlignedNoreturn(_))
            | Some(Token::AttributePacked)
            | Some(Token::AttributePackedAligned(_))
            | Some(Token::AttributePackedAlignedNoreturn(_))
            | Some(Token::AttributeTransparentUnion)
            | Some(Token::AttributeNoreturn)
            | Some(Token::AttributeNoInstrumentFunction)
            | Some(Token::AttributeMode(_))
            | Some(Token::AttributeVectorSize(_))
            | Some(Token::AttributeScalarStorageOrderReverse)
            | Some(Token::KWNoreturn) => true,
            Some(Token::Identifier(name)) => {
                name == "_BitInt"
                    || self.is_typedef_name(name)
                    || Self::is_builtin_int128_type_name(name)
                    || Self::is_complex_type_name(name)
                    || Self::is_builtin_float_type_name(name)
                    || Self::is_gnu_qualifier_name(name)
            }
            _ => false,
        }
    }

    pub(super) fn parse_block_item(&mut self) -> ParseResult<BlockItem> {
        if self.is_declaration_start() {
            if self.at(&Token::KWStaticAssert) {
                self.parse_static_assert_declaration()?;
                return Ok(BlockItem::Declaration(Declaration::TypedefDecl));
            }
            // Check for standalone enum declaration: enum Tag { ... };
            if self.at(&Token::KWEnum) {
                let save_pos = self.pos;
                self.advance()?;
                if let Some(Token::Identifier(_)) = self.peek() {
                    self.advance()?;
                }
                if self.at(&Token::OpenBrace) {
                    self.pos = save_pos;
                    let (_sc, _base_type) = self.parse_specifiers()?;
                    self.last_typedef_full_type = None;
                    if self.at(&Token::Semicolon) {
                        self.advance()?;
                        return Ok(BlockItem::Declaration(Declaration::TypedefDecl));
                    }
                    self.pos = save_pos;
                } else {
                    self.pos = save_pos;
                }
            }
            // Check for standalone struct/union declaration: struct tag { ... };
            if self.at(&Token::KWStruct) || self.at(&Token::KWUnion) {
                let is_union = self.at(&Token::KWUnion);
                let save_pos = self.pos;
                self.advance()?; // consume 'struct' or 'union'
                                 // Only check for standalone decl if next token is an identifier (not anonymous)
                let prefix_attrs = self.consume_aggregate_attributes()?;
                if let Some(Token::Identifier(_)) = self.peek() {
                    let tag = self.parse_identifier()?;
                    if self.at(&Token::OpenBrace) {
                        let members = self.parse_struct_members()?;
                        let member_vla_sizes = self
                            .pending_struct_member_vla_elem_sizes
                            .pop()
                            .unwrap_or_default();
                        let suffix_attrs = self.consume_aggregate_attributes()?;
                        self.validate_flexible_array_members(&members, is_union)?;
                        if self.at(&Token::Semicolon) {
                            self.advance()?;
                            let attrs =
                                Self::merge_aggregate_attributes(prefix_attrs, suffix_attrs);
                            let declaration = StructDeclaration {
                                tag: tag.clone(),
                                members,
                                is_union,
                                transparent_union: attrs.transparent_union,
                                packed: attrs.packed,
                                alignment: attrs.alignment,
                                reverse_storage_order: attrs.reverse_storage_order,
                            };
                            self.record_struct_definition(&declaration)?;
                            self.record_struct_member_vla_elem_sizes(&tag, member_vla_sizes);
                            return Ok(BlockItem::Declaration(Declaration::StructDecl(
                                declaration,
                            )));
                        }
                    } else if self.at(&Token::Semicolon) {
                        self.advance()?;
                        return Ok(BlockItem::Declaration(Declaration::StructDecl(
                            StructDeclaration {
                                tag,
                                members: vec![],
                                is_union,
                                transparent_union: prefix_attrs.transparent_union,
                                packed: prefix_attrs.packed,
                                alignment: prefix_attrs.alignment,
                                reverse_storage_order: prefix_attrs.reverse_storage_order,
                            },
                        )));
                    }
                }
                // Not a standalone decl — put back and let parse_specifiers handle it
                self.pos = save_pos;
            }
            let (mut sc, base_type) = self.parse_specifiers()?;
            sc = self.consume_post_type_storage_class(sc)?;
            let is_auto_type = std::mem::take(&mut self.pending_auto_type);
            let spec_noreturn = std::mem::take(&mut self.pending_noreturn);
            let spec_no_instrument = std::mem::take(&mut self.pending_no_instrument_function);
            let spec_inline = std::mem::take(&mut self.pending_inline);
            let decl_alignment = self.pending_alignment.take();
            let saved_struct_tag = if base_type == CType::Struct {
                self.last_struct_tag.clone()
            } else {
                None
            };
            let base_typedef_full_type = self.last_typedef_full_type.clone();
            let pre_attrs = self.consume_declaration_attributes()?;
            let decl_tree = self.parse_declarator_tree()?;
            let td_ft = base_typedef_full_type;
            self.last_typedef_full_type = None;
            let (name, full_type, decl_params) =
                Self::process_declarator(&decl_tree, base_type, td_ft.as_ref());
            let post_attrs = self.consume_declaration_attributes()?;
            let full_type = self.apply_vector_size_attr(
                full_type,
                post_attrs.vector_size.or(pre_attrs.vector_size),
            );
            let decl_alignment = [decl_alignment, pre_attrs.alignment, post_attrs.alignment]
                .into_iter()
                .flatten()
                .max();
            let decl_noreturn = spec_noreturn || pre_attrs.noreturn || post_attrs.noreturn;
            let decl_no_instrument =
                spec_no_instrument || std::mem::take(&mut self.pending_no_instrument_function);
            let decl_transparent_union = std::mem::take(&mut self.pending_transparent_union);
            // Replace Scalar(Struct) with FullType::Struct(tag)
            let full_type = if base_type == CType::Struct {
                if let Some(ref tag) = saved_struct_tag {
                    if decl_transparent_union {
                        self.mark_pending_transparent_union(tag);
                    }
                    Self::replace_scalar_struct(&full_type, tag)
                } else {
                    full_type
                }
            } else {
                full_type
            };

            if is_auto_type {
                if sc.as_ref().is_some_and(StorageClass::is_typedef)
                    || decl_params.is_some()
                    || full_type != FullType::Scalar(CType::Void)
                {
                    return Err(
                        self.format_error("__auto_type requires a single object declarator")
                    );
                }
                let decl = self.make_auto_var_decl(name, sc, decl_alignment)?;
                self.expect_token(Token::Semicolon)?;
                self.add_value_type(decl.name.clone(), self.var_decl_full_type(&decl)?)?;
                return Ok(BlockItem::Declaration(Declaration::VarDecl(decl)));
            }

            // Handle typedef declarations
            if sc.as_ref().is_some_and(StorageClass::is_typedef) {
                self.expect_token(Token::Semicolon)?;
                let vla_size = self.typedef_vla_size_expr(&full_type);
                self.add_typedef(
                    name,
                    TypedefInfo {
                        base_type: full_type.to_ctype(),
                        full_type: full_type.clone(),
                        struct_tag: saved_struct_tag,
                        is_enum: self.last_type_was_enum,
                        vla_size,
                        alignment: decl_alignment,
                    },
                )?;
                return Ok(BlockItem::Declaration(Declaration::TypedefDecl));
            }

            let ctype = full_type.to_ctype();
            let pi = match &full_type {
                FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                _ => None,
            };

            if decl_params.is_some() || self.at(&Token::OpenParen) {
                let func_info = if let Some(info) = decl_params {
                    info
                } else {
                    self.expect_token(Token::OpenParen)?;
                    let (
                        params,
                        param_full_types,
                        deprecated_params,
                        variadic,
                        zero_fixed_variadic,
                        old_style,
                        param_vla_bounds,
                    ) = self.parse_param_list()?;
                    self.expect_token(Token::CloseParen)?;
                    FunctionDeclaratorInfo {
                        params,
                        param_full_types,
                        deprecated_params,
                        variadic,
                        zero_fixed_variadic,
                        old_style,
                        param_vla_bounds,
                    }
                };
                let func_info = if self.at(&Token::Semicolon)
                    || self.at(&Token::OpenBrace)
                    || self.at(&Token::Comma)
                {
                    func_info
                } else {
                    self.parse_old_style_param_declarations(func_info)?
                };

                self.add_value_type(
                    name.clone(),
                    Self::function_full_type(full_type.clone(), &func_info),
                )?;
                let param_value_types =
                    Self::param_value_types(&func_info.params, &func_info.param_full_types);
                let body = if self.at(&Token::OpenBrace) {
                    Some(self.parse_function_body_preserving_type_decls(
                        &name,
                        &param_value_types,
                        &func_info.param_vla_bounds,
                    )?)
                } else {
                    if self.eat(&Token::Comma) {
                        let mut extra = Vec::new();
                        loop {
                            let decl_tree = self.parse_declarator_tree()?;
                            let (name2, full_type2, decl_params2) =
                                Self::process_declarator(&decl_tree, base_type, td_ft.as_ref());
                            let full_type2 =
                                self.apply_vector_size_attr(full_type2, post_attrs.vector_size);
                            let full_type2 = if base_type == CType::Struct {
                                if let Some(ref tag) = saved_struct_tag {
                                    Self::replace_scalar_struct(&full_type2, tag)
                                } else {
                                    full_type2
                                }
                            } else {
                                full_type2
                            };
                            let ctype2 = full_type2.to_ctype();
                            let pi2 = match &full_type2 {
                                FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                                _ => None,
                            };
                            if let Some(func_info2) = decl_params2 {
                                self.add_value_type(
                                    name2.clone(),
                                    Self::function_full_type(full_type2.clone(), &func_info2),
                                )?;
                                extra.push(BlockItem::Declaration(Declaration::FunDecl(
                                    FunctionDeclaration {
                                        name: name2,
                                        return_type: ctype2,
                                        return_ptr_info: pi2,
                                        return_full_type: Some(full_type2),
                                        params: func_info2.params,
                                        body: None,
                                        storage_class: sc.clone(),
                                        param_full_types: func_info2.param_full_types,
                                        param_vla_bounds: func_info2.param_vla_bounds,
                                        deprecated_params: func_info2.deprecated_params,
                                        variadic: func_info2.variadic,
                                        zero_fixed_variadic: func_info2.zero_fixed_variadic,
                                        old_style: func_info2.old_style,
                                        noreturn: decl_noreturn,
                                        no_instrument_function: decl_no_instrument,
                                        is_inline: spec_inline,
                                    },
                                )));
                            } else {
                                let decl = self.make_var_decl(
                                    name2,
                                    &full_type2,
                                    ctype2,
                                    pi2,
                                    sc.clone(),
                                    decl_alignment,
                                )?;
                                self.add_value_type(decl.name.clone(), full_type2.clone())?;
                                extra.push(BlockItem::Declaration(Declaration::VarDecl(decl)));
                            }
                            if !self.eat(&Token::Comma) {
                                break;
                            }
                        }
                        self.expect_token(Token::Semicolon)?;
                        self.pending_block_items.extend(extra);
                    } else {
                        self.expect_token(Token::Semicolon)?;
                    }
                    None
                };

                Ok(BlockItem::Declaration(Declaration::FunDecl(
                    FunctionDeclaration {
                        name,
                        return_type: ctype,
                        return_ptr_info: pi,
                        return_full_type: Some(full_type.clone()),
                        params: func_info.params,
                        body,
                        storage_class: sc,
                        param_full_types: func_info.param_full_types,
                        param_vla_bounds: func_info.param_vla_bounds,
                        deprecated_params: func_info.deprecated_params,
                        variadic: func_info.variadic,
                        zero_fixed_variadic: func_info.zero_fixed_variadic,
                        old_style: func_info.old_style,
                        noreturn: decl_noreturn,
                        no_instrument_function: decl_no_instrument,
                        is_inline: spec_inline,
                    },
                )))
            } else {
                let first =
                    self.make_var_decl(name, &full_type, ctype, pi, sc.clone(), decl_alignment)?;
                self.add_value_type(first.name.clone(), full_type.clone())?;
                // Check for multiple declarators: int x = 1, y, *z;
                if self.eat(&Token::Comma) {
                    let mut extra = Vec::new();
                    loop {
                        let decl_tree = self.parse_declarator_tree()?;
                        let (name2, full_type2, decl_params2) =
                            Self::process_declarator(&decl_tree, base_type, td_ft.as_ref());
                        let full_type2 = if base_type == CType::Struct {
                            if let Some(ref tag) = saved_struct_tag {
                                Self::replace_scalar_struct(&full_type2, tag)
                            } else {
                                full_type2
                            }
                        } else {
                            full_type2
                        };
                        let ctype2 = full_type2.to_ctype();
                        let pi2 = match &full_type2 {
                            FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                            _ => None,
                        };
                        if let Some(func_info2) = decl_params2 {
                            self.add_value_type(
                                name2.clone(),
                                Self::function_full_type(full_type2.clone(), &func_info2),
                            )?;
                            extra.push(BlockItem::Declaration(Declaration::FunDecl(
                                FunctionDeclaration {
                                    name: name2,
                                    return_type: ctype2,
                                    return_ptr_info: pi2,
                                    return_full_type: Some(full_type2),
                                    params: func_info2.params,
                                    body: None,
                                    storage_class: sc.clone(),
                                    param_full_types: func_info2.param_full_types,
                                    param_vla_bounds: func_info2.param_vla_bounds,
                                    deprecated_params: func_info2.deprecated_params,
                                    variadic: func_info2.variadic,
                                    zero_fixed_variadic: func_info2.zero_fixed_variadic,
                                    old_style: func_info2.old_style,
                                    noreturn: decl_noreturn,
                                    no_instrument_function: decl_no_instrument,
                                    is_inline: spec_inline,
                                },
                            )));
                        } else {
                            let decl = self.make_var_decl(
                                name2,
                                &full_type2,
                                ctype2,
                                pi2,
                                sc.clone(),
                                decl_alignment,
                            )?;
                            self.add_value_type(decl.name.clone(), full_type2.clone())?;
                            extra.push(BlockItem::Declaration(Declaration::VarDecl(decl)));
                        }
                        if !self.eat(&Token::Comma) {
                            break;
                        }
                    }
                    self.expect_token(Token::Semicolon)?;
                    // Stash extras for parse_block to collect
                    self.pending_block_items.extend(extra);
                } else {
                    self.expect_token(Token::Semicolon)?;
                }
                Ok(BlockItem::Declaration(Declaration::VarDecl(first)))
            }
        } else {
            Ok(BlockItem::Statement(self.parse_statement()?))
        }
    }

    // --------------------------------------------------------
    // Statements
    // --------------------------------------------------------

    pub(super) fn parse_statement(&mut self) -> ParseResult<Statement> {
        match self.peek() {
            Some(Token::KWReturn) => {
                self.advance()?;
                if self.at(&Token::Semicolon) {
                    self.advance()?;
                    Ok(Statement::Return(None))
                } else {
                    let exp = self.parse_expression()?;
                    self.expect_token(Token::Semicolon)?;
                    Ok(Statement::Return(Some(exp)))
                }
            }
            Some(Token::KWIf) => {
                self.advance()?;
                self.expect_token(Token::OpenParen)?;
                let condition = self.parse_expression()?;
                self.expect_token(Token::CloseParen)?;
                let then_stmt = Box::new(self.parse_statement()?);
                let else_stmt = if self.eat(&Token::KWElse) {
                    Some(Box::new(self.parse_statement()?))
                } else {
                    None
                };
                Ok(Statement::If(condition, then_stmt, else_stmt))
            }
            Some(Token::KWWhile) => {
                self.advance()?;
                self.expect_token(Token::OpenParen)?;
                let condition = self.parse_expression()?;
                self.expect_token(Token::CloseParen)?;
                let body = Box::new(self.parse_statement()?);
                Ok(Statement::While {
                    condition,
                    body,
                    label: String::new(), // filled by resolve pass
                })
            }
            Some(Token::KWDo) => {
                self.advance()?;
                let body = Box::new(self.parse_statement()?);
                self.expect_token(Token::KWWhile)?;
                self.expect_token(Token::OpenParen)?;
                let condition = self.parse_expression()?;
                self.expect_token(Token::CloseParen)?;
                self.expect_token(Token::Semicolon)?;
                Ok(Statement::DoWhile {
                    body,
                    condition,
                    label: String::new(),
                })
            }
            Some(Token::KWFor) => {
                self.advance()?;
                self.expect_token(Token::OpenParen)?;
                let init = if self.is_declaration_start() {
                    ForInit::Declaration(self.parse_var_declaration()?)
                } else if self.eat(&Token::Semicolon) {
                    ForInit::Expression(None)
                } else {
                    let exp = self.parse_expression()?;
                    self.expect_token(Token::Semicolon)?;
                    ForInit::Expression(Some(exp))
                };
                let condition = if self.at(&Token::Semicolon) {
                    None
                } else {
                    Some(self.parse_expression()?)
                };
                self.expect_token(Token::Semicolon)?;
                let post = if self.at(&Token::CloseParen) {
                    None
                } else {
                    Some(self.parse_expression()?)
                };
                self.expect_token(Token::CloseParen)?;
                let body = Box::new(self.parse_statement()?);
                Ok(Statement::For {
                    init: Box::new(init),
                    condition,
                    post,
                    body,
                    label: String::new(),
                })
            }
            Some(Token::KWBreak) => {
                self.advance()?;
                self.expect_token(Token::Semicolon)?;
                Ok(Statement::Break(String::new())) // filled by resolve pass
            }
            Some(Token::KWContinue) => {
                self.advance()?;
                self.expect_token(Token::Semicolon)?;
                Ok(Statement::Continue(String::new()))
            }
            Some(Token::KWGoto) => {
                self.advance()?;
                if self.eat(&Token::Star) {
                    let target = self.parse_expression()?;
                    self.expect_token(Token::Semicolon)?;
                    Ok(Statement::IndirectGoto(target))
                } else {
                    let label = self.parse_identifier()?;
                    self.expect_token(Token::Semicolon)?;
                    Ok(Statement::Goto(label))
                }
            }
            Some(Token::KWSwitch) => {
                self.advance()?;
                self.expect_token(Token::OpenParen)?;
                let control = self.parse_expression()?;
                self.expect_token(Token::CloseParen)?;
                let body = Box::new(self.parse_statement()?);
                Ok(Statement::Switch {
                    control,
                    body,
                    label: String::new(),
                    cases: Vec::new(),
                })
            }
            Some(Token::KWCase) => {
                self.advance()?;
                let value = self.parse_expression()?;
                let end_value = if self.eat(&Token::Ellipsis) {
                    Some(self.parse_expression()?)
                } else {
                    None
                };
                self.expect_token(Token::Colon)?;
                let body = if self.at(&Token::CloseBrace) {
                    Box::new(Statement::Null)
                } else {
                    Box::new(self.parse_statement()?)
                };
                Ok(Statement::Case {
                    value,
                    end_value,
                    body,
                    label: String::new(),
                })
            }
            Some(Token::KWDefault) => {
                self.advance()?;
                self.expect_token(Token::Colon)?;
                let body = if self.at(&Token::CloseBrace) {
                    Box::new(Statement::Null)
                } else {
                    Box::new(self.parse_statement()?)
                };
                Ok(Statement::Default {
                    body,
                    label: String::new(),
                })
            }
            Some(Token::OpenBrace) => Ok(Statement::Block(self.parse_block()?)),
            Some(Token::Semicolon) => {
                self.advance()?;
                Ok(Statement::Null)
            }
            Some(Token::Identifier(name)) if name == "__label__" => {
                self.advance()?;
                loop {
                    self.parse_identifier()?;
                    if !self.eat(&Token::Comma) {
                        break;
                    }
                }
                self.expect_token(Token::Semicolon)?;
                Ok(Statement::Null)
            }
            // Check for labeled statement: identifier ':'
            Some(Token::Identifier(_))
                if self.pos + 1 < self.tokens.len()
                    && self.tokens[self.pos + 1] == Token::Colon =>
            {
                let name = self.parse_identifier()?;
                self.expect_token(Token::Colon)?;
                let stmt = if self.at(&Token::CloseBrace) {
                    Box::new(Statement::Null)
                } else {
                    Box::new(self.parse_statement()?)
                };
                Ok(Statement::Label(name, stmt))
            }
            _ => {
                let exp = self.parse_expression()?;
                self.expect_token(Token::Semicolon)?;
                Ok(Statement::Expression(exp))
            }
        }
    }

    // --------------------------------------------------------
    // Expressions (precedence climbing)
    // --------------------------------------------------------

    pub(super) fn parse_expression(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_assignment()?;
        while self.eat(&Token::Comma) {
            let right = self.parse_assignment()?;
            left = Exp::Comma(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    pub(super) fn parse_assignment(&mut self) -> ParseResult<Exp> {
        let left = self.parse_conditional()?;

        match self.peek().cloned() {
            Some(Token::Assign) => {
                self.advance()?;
                let right = self.parse_assignment()?; // right-associative
                Ok(Exp::Assign(Box::new(left), Box::new(right)))
            }
            Some(tok) => {
                if let Some(op) = Self::compound_assign_op(&tok) {
                    self.advance()?;
                    let right = self.parse_assignment()?;
                    Ok(Exp::CompoundAssign(op, Box::new(left), Box::new(right)))
                } else {
                    Ok(left)
                }
            }
            None => Ok(left),
        }
    }

    pub(super) fn compound_assign_op(tok: &Token) -> Option<BinaryOp> {
        match tok {
            Token::PlusAssign => Some(BinaryOp::Add),
            Token::MinusAssign => Some(BinaryOp::Sub),
            Token::StarAssign => Some(BinaryOp::Mul),
            Token::SlashAssign => Some(BinaryOp::Div),
            Token::PercentAssign => Some(BinaryOp::Mod),
            Token::AmpersandAssign => Some(BinaryOp::BitwiseAnd),
            Token::PipeAssign => Some(BinaryOp::BitwiseOr),
            Token::CaretAssign => Some(BinaryOp::BitwiseXor),
            Token::ShiftLeftAssign => Some(BinaryOp::ShiftLeft),
            Token::ShiftRightAssign => Some(BinaryOp::ShiftRight),
            _ => None,
        }
    }

    pub(super) fn parse_conditional(&mut self) -> ParseResult<Exp> {
        let cond = self.parse_logical_or()?;
        if self.eat(&Token::Question) {
            let then_exp = if self.at(&Token::Colon) {
                cond.clone()
            } else {
                self.parse_expression()?
            };
            self.expect_token(Token::Colon)?;
            let else_exp = self.parse_conditional()?; // right-associative
            Ok(Exp::Conditional(
                Box::new(cond),
                Box::new(then_exp),
                Box::new(else_exp),
            ))
        } else {
            Ok(cond)
        }
    }

    pub(super) fn parse_logical_or(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_logical_and()?;
        while self.eat(&Token::LogicalOr) {
            let right = self.parse_logical_and()?;
            left = Exp::Binary(BinaryOp::LogicalOr, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    pub(super) fn parse_logical_and(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_bitwise_or()?;
        while self.eat(&Token::LogicalAnd) {
            let right = self.parse_bitwise_or()?;
            left = Exp::Binary(BinaryOp::LogicalAnd, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    pub(super) fn parse_bitwise_or(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_bitwise_xor()?;
        while self.eat(&Token::Pipe) {
            let right = self.parse_bitwise_xor()?;
            left = Exp::Binary(BinaryOp::BitwiseOr, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    pub(super) fn parse_bitwise_xor(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_bitwise_and()?;
        while self.eat(&Token::Caret) {
            let right = self.parse_bitwise_and()?;
            left = Exp::Binary(BinaryOp::BitwiseXor, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    pub(super) fn parse_bitwise_and(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_equality()?;
        while self.eat(&Token::Ampersand) {
            let right = self.parse_equality()?;
            left = Exp::Binary(BinaryOp::BitwiseAnd, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    pub(super) fn parse_equality(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_relational()?;
        loop {
            match self.peek().cloned() {
                Some(Token::EqualEqual) => {
                    self.advance()?;
                    let right = self.parse_relational()?;
                    left = Exp::Binary(BinaryOp::Equal, Box::new(left), Box::new(right));
                }
                Some(Token::NotEqual) => {
                    self.advance()?;
                    let right = self.parse_relational()?;
                    left = Exp::Binary(BinaryOp::NotEqual, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    pub(super) fn parse_relational(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_shift()?;
        loop {
            match self.peek().cloned() {
                Some(Token::LessThan) => {
                    self.advance()?;
                    let right = self.parse_shift()?;
                    left = Exp::Binary(BinaryOp::LessThan, Box::new(left), Box::new(right));
                }
                Some(Token::GreaterThan) => {
                    self.advance()?;
                    let right = self.parse_shift()?;
                    left = Exp::Binary(BinaryOp::GreaterThan, Box::new(left), Box::new(right));
                }
                Some(Token::LessEqual) => {
                    self.advance()?;
                    let right = self.parse_shift()?;
                    left = Exp::Binary(BinaryOp::LessEqual, Box::new(left), Box::new(right));
                }
                Some(Token::GreaterEqual) => {
                    self.advance()?;
                    let right = self.parse_shift()?;
                    left = Exp::Binary(BinaryOp::GreaterEqual, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    pub(super) fn parse_shift(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_additive()?;
        loop {
            match self.peek().cloned() {
                Some(Token::ShiftLeft) => {
                    self.advance()?;
                    let right = self.parse_additive()?;
                    left = Exp::Binary(BinaryOp::ShiftLeft, Box::new(left), Box::new(right));
                }
                Some(Token::ShiftRight) => {
                    self.advance()?;
                    let right = self.parse_additive()?;
                    left = Exp::Binary(BinaryOp::ShiftRight, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    pub(super) fn parse_additive(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_multiplicative()?;
        loop {
            match self.peek().cloned() {
                Some(Token::Plus) => {
                    self.advance()?;
                    let right = self.parse_multiplicative()?;
                    left = Exp::Binary(BinaryOp::Add, Box::new(left), Box::new(right));
                }
                Some(Token::Minus) => {
                    self.advance()?;
                    let right = self.parse_multiplicative()?;
                    left = Exp::Binary(BinaryOp::Sub, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    pub(super) fn parse_multiplicative(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_unary()?;
        loop {
            match self.peek().cloned() {
                Some(Token::Star) => {
                    self.advance()?;
                    let right = self.parse_unary()?;
                    left = Exp::Binary(BinaryOp::Mul, Box::new(left), Box::new(right));
                }
                Some(Token::Slash) => {
                    self.advance()?;
                    let right = self.parse_unary()?;
                    left = Exp::Binary(BinaryOp::Div, Box::new(left), Box::new(right));
                }
                Some(Token::Percent) => {
                    self.advance()?;
                    let right = self.parse_unary()?;
                    left = Exp::Binary(BinaryOp::Mod, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    pub(super) fn parse_unary(&mut self) -> ParseResult<Exp> {
        match self.peek().cloned() {
            Some(Token::Plus) => {
                self.advance()?;
                self.parse_unary()
            }
            Some(Token::Minus) => {
                self.advance()?;
                Ok(Exp::Unary(UnaryOp::Negate, Box::new(self.parse_unary()?)))
            }
            Some(Token::Tilde) => {
                self.advance()?;
                Ok(Exp::Unary(
                    UnaryOp::Complement,
                    Box::new(self.parse_unary()?),
                ))
            }
            Some(Token::Bang) => {
                self.advance()?;
                Ok(Exp::Unary(
                    UnaryOp::LogicalNot,
                    Box::new(self.parse_unary()?),
                ))
            }
            Some(Token::Increment) => {
                self.advance()?;
                Ok(Exp::Unary(
                    UnaryOp::PreIncrement,
                    Box::new(self.parse_unary()?),
                ))
            }
            Some(Token::Decrement) => {
                self.advance()?;
                Ok(Exp::Unary(
                    UnaryOp::PreDecrement,
                    Box::new(self.parse_unary()?),
                ))
            }
            Some(Token::Star) => {
                self.advance()?;
                Ok(Exp::Unary(UnaryOp::Deref, Box::new(self.parse_unary()?)))
            }
            Some(Token::Ampersand) => {
                self.advance()?;
                Ok(Exp::Unary(UnaryOp::AddrOf, Box::new(self.parse_unary()?)))
            }
            Some(Token::Identifier(name))
                if matches!(name.as_str(), "__real__" | "__imag__" | "__real" | "__imag") =>
            {
                let op = if matches!(name.as_str(), "__real__" | "__real") {
                    UnaryOp::RealPart
                } else {
                    UnaryOp::ImagPart
                };
                self.advance()?;
                Ok(Exp::Unary(op, Box::new(self.parse_unary()?)))
            }
            // sizeof expression or sizeof(type)
            Some(Token::KWSizeOf) => {
                self.advance()?;
                // sizeof(type) — check for ( followed by type keyword
                if self.at(&Token::OpenParen)
                    && self.pos + 1 < self.tokens.len()
                    && self.is_type_keyword(&self.tokens[self.pos + 1])
                {
                    self.advance()?; // consume '('
                    let full_type = self.parse_type_name_full()?;
                    let vla_size = self
                        .last_type_name_vla_size
                        .take()
                        .or_else(|| self.pending_vla_size_expr_for_type(&full_type))
                        .or_else(|| self.dynamic_size_expr_for_full_type(&full_type));
                    self.expect_token(Token::CloseParen)?;
                    if let Some(size) = vla_size {
                        return Ok(size);
                    }
                    if self.at(&Token::OpenBrace) {
                        let _init = self.parse_array_init()?;
                        let ctype = full_type.to_ctype();
                        return Ok(Exp::SizeOfType(ctype, full_type));
                    }
                    let ctype = full_type.to_ctype();
                    Ok(Exp::SizeOfType(ctype, full_type))
                } else {
                    // sizeof <unary-exp> (not a cast expression)
                    let operand = self.parse_unary()?;
                    if let Exp::Var(name) = &operand {
                        if let Some(size) = self.lookup_value_vla_size(name) {
                            return Ok(size);
                        }
                    }
                    if let Exp::Subscript(arr, _) = &operand {
                        if let Exp::Var(name) = arr.as_ref() {
                            if let Some(size) = self.lookup_value_vla_elem_size(name) {
                                return Ok(size);
                            }
                        }
                    }
                    Ok(Exp::SizeOf(Box::new(operand)))
                }
            }
            Some(Token::KWAlignOf) => {
                self.advance()?;
                if self.at(&Token::OpenParen)
                    && self.pos + 1 < self.tokens.len()
                    && self.is_type_keyword(&self.tokens[self.pos + 1])
                {
                    self.advance()?;
                    let full_type = self.parse_type_name_full()?;
                    self.expect_token(Token::CloseParen)?;
                    Ok(Exp::AlignOfType(full_type))
                } else {
                    let operand = self.parse_unary()?;
                    if let Exp::Var(name) = &operand {
                        if let Some(alignment) = self.function_alignments.get(name) {
                            let alignment = i64::try_from(*alignment).map_err(|_| {
                                self.format_error("function alignment exceeds i64 range")
                            })?;
                            return Ok(Exp::Constant(alignment));
                        }
                    }
                    let full_type = self.typeof_expression(&operand)?;
                    Ok(Exp::AlignOfType(full_type))
                }
            }
            // Cast expression or compound literal: (type) unary or (type){init}
            Some(Token::OpenParen)
                if self.pos + 1 < self.tokens.len()
                    && self.is_type_keyword(&self.tokens[self.pos + 1]) =>
            {
                self.advance()?; // consume '('
                let full_type = self.parse_type_name_full()?;
                self.expect_token(Token::CloseParen)?;
                if self.at(&Token::OpenBrace) {
                    // Compound literal: (Type){init}
                    let init = self.parse_array_init()?;
                    // Treat as a cast of the initializer to the target type
                    let target_type = full_type.to_ctype();
                    self.parse_postfix_suffix(Exp::Cast(
                        target_type,
                        Some(full_type),
                        Box::new(init),
                    ))
                } else {
                    let target_type = full_type.to_ctype();
                    let operand = self.parse_unary()?;
                    let cast_ft = if target_type == CType::Pointer
                        || target_type == CType::Struct
                        || full_type.is_vector()
                    {
                        Some(full_type)
                    } else {
                        None
                    };
                    Ok(Exp::Cast(target_type, cast_ft, Box::new(operand)))
                }
            }
            _ => self.parse_postfix(),
        }
    }

    pub(super) fn parse_postfix(&mut self) -> ParseResult<Exp> {
        let expr = self.parse_primary()?;
        self.parse_postfix_suffix(expr)
    }

    pub(super) fn parse_postfix_suffix(&mut self, mut expr: Exp) -> ParseResult<Exp> {
        loop {
            match self.peek().cloned() {
                Some(Token::Increment) => {
                    self.advance()?;
                    expr = Exp::Unary(UnaryOp::PostIncrement, Box::new(expr));
                }
                Some(Token::Decrement) => {
                    self.advance()?;
                    expr = Exp::Unary(UnaryOp::PostDecrement, Box::new(expr));
                }
                Some(Token::OpenBracket) => {
                    self.advance()?;
                    let index = self.parse_expression()?;
                    self.expect_token(Token::CloseBracket)?;
                    expr = Exp::Subscript(Box::new(expr), Box::new(index));
                }
                Some(Token::Dot) => {
                    self.advance()?;
                    let member = self.parse_identifier()?;
                    expr = Exp::Dot(Box::new(expr), member);
                }
                Some(Token::Arrow) => {
                    self.advance()?;
                    let member = self.parse_identifier()?;
                    expr = Exp::Arrow(Box::new(expr), member);
                }
                Some(Token::OpenParen) if !matches!(expr, Exp::Var(_)) => {
                    // Indirect call through expression: expr(args)
                    // e.g., ops[0](1,2) or get_func()(args)
                    self.advance()?;
                    let args = self.parse_arg_list()?;
                    self.expect_token(Token::CloseParen)?;
                    expr = Exp::IndirectCall(Box::new(expr), args);
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    pub(super) fn generic_types_match(control: &FullType, association: &FullType) -> bool {
        let control = Self::generic_decay_type(control);
        let association = Self::generic_decay_type(association);
        control == association
            || matches!(
                (&control, &association),
                (FullType::Scalar(left), FullType::Scalar(right)) if left == right
            )
    }

    pub(super) fn generic_decay_type(full_type: &FullType) -> FullType {
        match full_type {
            FullType::Function { .. } => FullType::Pointer(Box::new(full_type.clone())),
            _ => full_type.decay(),
        }
    }

    pub(super) fn parse_generic_selection(&mut self) -> ParseResult<Exp> {
        self.expect_token(Token::KWGeneric)?;
        self.expect_token(Token::OpenParen)?;
        let control_type = if self.is_type_keyword_at_pos() {
            self.parse_type_name_full()?
        } else {
            let control = self.parse_assignment()?;
            self.typeof_expression(&control)?
        };
        self.expect_token(Token::Comma)?;

        let mut selected = None;
        let mut default = None;
        loop {
            if self.eat(&Token::KWDefault) {
                if default.is_some() {
                    return Err(self.format_error("duplicate default generic association"));
                }
                self.expect_token(Token::Colon)?;
                let expr = self.parse_assignment()?;
                default = Some(expr);
            } else {
                let assoc_type = self.parse_type_name_full()?;
                self.expect_token(Token::Colon)?;
                let expr = self.parse_assignment()?;
                if selected.is_none() && Self::generic_types_match(&control_type, &assoc_type) {
                    selected = Some(expr);
                }
            }

            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect_token(Token::CloseParen)?;
        selected
            .or(default)
            .ok_or_else(|| self.format_error("no matching generic association"))
    }

    pub(super) fn parse_primary(&mut self) -> ParseResult<Exp> {
        match self.peek().cloned() {
            Some(Token::LogicalAnd) => {
                self.advance()?;
                let label = self.parse_identifier()?;
                Ok(Exp::LabelAddress(label))
            }
            Some(Token::KWGeneric) => self.parse_generic_selection(),
            Some(Token::IntLiteral(val)) => {
                self.advance()?;
                if val >= i32::MIN as i64 && val <= i32::MAX as i64 {
                    Ok(Exp::Constant(val))
                } else {
                    Ok(Exp::LongConstant(val))
                }
            }
            Some(Token::LongLiteral(val)) => {
                self.advance()?;
                Ok(Exp::LongConstant(val))
            }
            Some(Token::Int128Literal(val)) => {
                self.advance()?;
                Ok(Exp::Int128Constant(val))
            }
            Some(Token::UIntLiteral(val)) => {
                self.advance()?;
                // UInt constants > UINT_MAX are promoted to ulong
                if val > u32::MAX as i64 {
                    Ok(Exp::ULongConstant(val))
                } else {
                    Ok(Exp::UIntConstant(val))
                }
            }
            Some(Token::ULongLiteral(val)) => {
                self.advance()?;
                Ok(Exp::ULongConstant(val))
            }
            Some(Token::UInt128Literal(val)) => {
                self.advance()?;
                Ok(Exp::UInt128Constant(val))
            }
            Some(Token::DoubleLiteral(val)) => {
                self.advance()?;
                Ok(Exp::DoubleConstant(val))
            }
            Some(Token::LongDoubleLiteral(val)) => {
                self.advance()?;
                Ok(Exp::LongDoubleConstant(val))
            }
            Some(Token::ImaginaryIntLiteral(val)) => {
                self.advance()?;
                Ok(Exp::ImaginaryIntConstant(val))
            }
            Some(Token::ImaginaryDoubleLiteral(val)) => {
                self.advance()?;
                Ok(Exp::ImaginaryDoubleConstant(val))
            }
            Some(Token::CharLiteral(val)) => {
                self.advance()?;
                Ok(Exp::Constant(val)) // char constants have type int
            }
            Some(
                Token::StringLiteral(_)
                | Token::WideStringLiteral(_)
                | Token::Utf16StringLiteral(_)
                | Token::Utf32StringLiteral(_),
            ) => {
                // Concatenate adjacent string literals. Narrow pieces can join
                // any prefixed literal; distinct non-narrow prefixes cannot.
                let mut s = String::new();
                let mut kind: Option<&'static str> = None;
                while matches!(
                    self.peek(),
                    Some(
                        Token::StringLiteral(_)
                            | Token::WideStringLiteral(_)
                            | Token::Utf16StringLiteral(_)
                            | Token::Utf32StringLiteral(_)
                    )
                ) {
                    match self.peek().cloned() {
                        Some(Token::StringLiteral(part)) => {
                            self.advance()?;
                            s.push_str(&part);
                        }
                        Some(Token::WideStringLiteral(part)) => {
                            self.advance()?;
                            if let Some(existing) = kind {
                                if existing != "wide" {
                                    return Err(self.format_error(
                                        "cannot concatenate differently-prefixed string literals",
                                    ));
                                }
                            }
                            kind = Some("wide");
                            s.push_str(&part);
                        }
                        Some(Token::Utf16StringLiteral(part)) => {
                            self.advance()?;
                            if let Some(existing) = kind {
                                if existing != "utf16" {
                                    return Err(self.format_error(
                                        "cannot concatenate differently-prefixed string literals",
                                    ));
                                }
                            }
                            kind = Some("utf16");
                            s.push_str(&part);
                        }
                        Some(Token::Utf32StringLiteral(part)) => {
                            self.advance()?;
                            if let Some(existing) = kind {
                                if existing != "utf32" {
                                    return Err(self.format_error(
                                        "cannot concatenate differently-prefixed string literals",
                                    ));
                                }
                            }
                            kind = Some("utf32");
                            s.push_str(&part);
                        }
                        _ => {
                            return Err(self.format_error("unexpected string literal state"));
                        }
                    }
                }
                match kind {
                    Some("wide") => Ok(Exp::WideStringLiteral(s)),
                    Some("utf16") => Ok(Exp::Utf16StringLiteral(s)),
                    Some("utf32") => Ok(Exp::Utf32StringLiteral(s)),
                    Some(_) => Err(self.format_error("unexpected string literal prefix state")),
                    None => Ok(Exp::StringLiteral(s)),
                }
            }
            Some(Token::Identifier(name)) => {
                self.advance()?;
                if matches!(
                    name.as_str(),
                    "__FUNCTION__" | "__PRETTY_FUNCTION__" | "__func__"
                ) {
                    return Ok(Exp::StringLiteral(
                        self.current_function_name.clone().unwrap_or_default(),
                    ));
                }
                // Check for function call
                if self.eat(&Token::OpenParen) {
                    if name == "__builtin_types_compatible_p" {
                        let left_start = self.pos;
                        let left = self.parse_type_name_full()?;
                        let left_tokens = self.tokens[left_start..self.pos].to_vec();
                        self.expect_token(Token::Comma)?;
                        let right_start = self.pos;
                        let right = self.parse_type_name_full()?;
                        let right_tokens = self.tokens[right_start..self.pos].to_vec();
                        self.expect_token(Token::CloseParen)?;
                        let compatible = Self::gnu_types_compatible(&left, &right)
                            && Self::gnu_type_meta_compatible(
                                &self.compat_type_meta(&left, &left_tokens),
                                &self.compat_type_meta(&right, &right_tokens),
                            );
                        return Ok(Exp::Constant(compatible as i64));
                    }
                    if name == "__builtin_offsetof" {
                        let full_type = self.parse_type_name_full()?;
                        self.expect_token(Token::Comma)?;
                        let offset = self.offsetof_member_designator(full_type)?;
                        self.expect_token(Token::CloseParen)?;
                        return Ok(offset);
                    }
                    if name == "__builtin_va_arg" {
                        let ap = self.parse_assignment()?;
                        self.expect_token(Token::Comma)?;
                        let ty = self.parse_type()?;
                        let parsed_full_type = self.last_typedef_full_type.take();
                        let struct_tag = if ty == CType::Struct {
                            self.last_struct_tag.clone()
                        } else {
                            None
                        };
                        let abstract_full_type = self.parse_abstract_declarator_type(ty)?;
                        let full_type = parsed_full_type.unwrap_or(abstract_full_type);
                        let full_type = if ty == CType::Struct {
                            if let Some(ref tag) = struct_tag {
                                Self::replace_scalar_struct(&full_type, tag)
                            } else {
                                full_type
                            }
                        } else {
                            full_type
                        };
                        if full_type == FullType::Scalar(CType::Void) {
                            return Err(
                                self.format_error("__builtin_va_arg cannot read a void value")
                            );
                        }
                        self.expect_token(Token::CloseParen)?;
                        let helper = match &full_type {
                            FullType::Struct(tag) => format!("__rnqcc_va_arg_struct_{}", tag),
                            _ => {
                                let suffix = match full_type.to_ctype() {
                                    CType::Long => "long",
                                    CType::ULong => "ulong",
                                    CType::Pointer => "ptr",
                                    CType::UInt => "uint",
                                    CType::Short => "short",
                                    CType::UShort => "ushort",
                                    CType::Char | CType::SChar => "char",
                                    CType::UChar => "uchar",
                                    CType::Float => "float",
                                    CType::Double => "double",
                                    CType::LongDouble => "long_double",
                                    CType::Int128 => "int128",
                                    CType::UInt128 => "uint128",
                                    _ => "int",
                                };
                                format!("__rnqcc_va_arg_{}", suffix)
                            }
                        };
                        let call = Exp::FunctionCall(helper, vec![ap]);
                        if full_type.is_pointer() {
                            return Ok(Exp::Cast(
                                full_type.to_ctype(),
                                Some(full_type),
                                Box::new(call),
                            ));
                        }
                        return Ok(call);
                    }
                    let args = self.parse_arg_list()?;
                    self.expect_token(Token::CloseParen)?;
                    if name == "__builtin_expect" || name == "__builtin_expect_with_probability" {
                        let expected_args = if name == "__builtin_expect" { 2 } else { 3 };
                        if args.len() != expected_args {
                            return Err(self.format_error(&format!(
                                "{} requires exactly {} arguments",
                                name, expected_args
                            )));
                        }
                        let mut args = args;
                        let value = args.drain(..1).next().ok_or_else(|| {
                            self.format_error(&format!("{} requires an argument", name))
                        })?;
                        return Ok(Exp::BuiltinExpect(Box::new(value), args));
                    }
                    if name == "__builtin_constant_p" {
                        if args.len() > 1 {
                            return Err(self.format_error(
                                "__builtin_constant_p requires exactly one argument",
                            ));
                        }
                        let Some(arg) = args.first() else {
                            return Err(
                                self.format_error("__builtin_constant_p requires an argument")
                            );
                        };
                        let is_constant = self.eval_integer_constant_exp_with_layout(arg).is_some()
                            || matches!(
                                arg,
                                Exp::DoubleConstant(_)
                                    | Exp::LongDoubleConstant(_)
                                    | Exp::StringLiteral(_)
                            );
                        return Ok(Exp::Constant(is_constant as i64));
                    }
                    if name == "__builtin_classify_type" {
                        if args.len() > 1 {
                            return Err(self.format_error(
                                "__builtin_classify_type requires exactly one argument",
                            ));
                        }
                        let Some(arg) = args.first() else {
                            return Err(
                                self.format_error("__builtin_classify_type requires an argument")
                            );
                        };
                        let ty = self.typeof_expression(arg)?.to_ctype();
                        return Ok(Exp::Constant(if ty.is_floating() { 8 } else { 1 }));
                    }
                    if name == "__builtin_signbit" {
                        if args.len() > 1 {
                            return Err(self
                                .format_error("__builtin_signbit requires exactly one argument"));
                        }
                        let Some(arg) = args.first() else {
                            return Err(self.format_error("__builtin_signbit requires an argument"));
                        };
                        let reciprocal_is_negative = Exp::Binary(
                            BinaryOp::LessThan,
                            Box::new(Exp::Binary(
                                BinaryOp::Div,
                                Box::new(Exp::DoubleConstant(1.0)),
                                Box::new(arg.clone()),
                            )),
                            Box::new(Exp::DoubleConstant(0.0)),
                        );
                        let value_is_negative = Exp::Binary(
                            BinaryOp::LessThan,
                            Box::new(arg.clone()),
                            Box::new(Exp::DoubleConstant(0.0)),
                        );
                        return Ok(Exp::Binary(
                            BinaryOp::LogicalOr,
                            Box::new(reciprocal_is_negative),
                            Box::new(value_is_negative),
                        ));
                    }
                    if name == "__builtin_strlen" {
                        if args.len() > 1 {
                            return Err(
                                self.format_error("__builtin_strlen requires exactly one argument")
                            );
                        }
                        let Some(arg) = args.first() else {
                            return Err(self.format_error("__builtin_strlen requires an argument"));
                        };
                        if let Exp::StringLiteral(s) = arg {
                            let length = i64::try_from(c_string_byte_len(s)).map_err(|_| {
                                self.format_error("__builtin_strlen result exceeds i64 range")
                            })?;
                            return Ok(Exp::ULongConstant(length));
                        }
                    }
                    if let Some(fallback) = Self::fortified_builtin_fallback(&name, &args) {
                        return fallback.map_err(|err| self.format_error(&err));
                    }
                    if matches!(
                        name.as_str(),
                        "__builtin_object_size" | "__builtin_dynamic_object_size"
                    ) {
                        if args.len() != 2 {
                            return Err(self.format_error(&format!(
                                "{} requires exactly two arguments",
                                name
                            )));
                        }
                        let mode = self
                            .eval_integer_constant_exp_with_layout(&args[1])
                            .ok_or_else(|| {
                                self.format_error(&format!(
                                    "{} mode argument must be an integer constant",
                                    name
                                ))
                            })?;
                        if !(0..=3).contains(&mode) {
                            return Err(self
                                .format_error(&format!("{} mode must be between 0 and 3", name)));
                        }
                        return Ok(if mode >= 2 {
                            Exp::ULongConstant(0)
                        } else {
                            Exp::ULongConstant(-1)
                        });
                    }
                    if name == "__builtin_assume_aligned" {
                        if !(2..=3).contains(&args.len()) {
                            return Err(self.format_error(
                                "__builtin_assume_aligned requires two or three arguments",
                            ));
                        }
                        let value = args[0].clone();
                        let mut result = value;
                        for arg in args[1..].iter().rev() {
                            result = Exp::Comma(Box::new(arg.clone()), Box::new(result));
                        }
                        return Ok(result);
                    }
                    if name == "__builtin_prefetch" {
                        if !(1..=3).contains(&args.len()) {
                            return Err(self.format_error(
                                "__builtin_prefetch requires one to three arguments",
                            ));
                        }
                        let result = args.into_iter().fold(Exp::Constant(0), |result, arg| {
                            Exp::Comma(Box::new(arg), Box::new(result))
                        });
                        return Ok(result);
                    }
                    if name == "__builtin_va_end" {
                        if args.len() != 1 {
                            return Err(
                                self.format_error("__builtin_va_end requires exactly one argument")
                            );
                        }
                        let arg = args.into_iter().next().ok_or_else(|| {
                            self.format_error("__builtin_va_end requires exactly one argument")
                        })?;
                        return Ok(Exp::Comma(Box::new(arg), Box::new(Exp::Constant(0))));
                    }
                    if matches!(
                        name.as_str(),
                        "__atomic_thread_fence" | "__atomic_signal_fence"
                    ) {
                        if args.len() != 1 {
                            return Err(self
                                .format_error(&format!("{} requires exactly one argument", name)));
                        }
                        return Ok(Exp::AtomicFence);
                    }
                    if name == "__sync_synchronize" {
                        if !args.is_empty() {
                            return Err(
                                self.format_error("__sync_synchronize requires no arguments")
                            );
                        }
                        return Ok(Exp::AtomicFence);
                    }
                    if name == "__builtin_bswap32" {
                        if args.len() > 1 {
                            return Err(self
                                .format_error("__builtin_bswap32 requires exactly one argument"));
                        }
                        let Some(arg) = args.first() else {
                            return Err(self.format_error("__builtin_bswap32 requires an argument"));
                        };
                        return Ok(Self::bswap_exp(arg.clone(), 32));
                    }
                    if name == "__builtin_bswap64" {
                        if args.len() > 1 {
                            return Err(self
                                .format_error("__builtin_bswap64 requires exactly one argument"));
                        }
                        let Some(arg) = args.first() else {
                            return Err(self.format_error("__builtin_bswap64 requires an argument"));
                        };
                        return Ok(Self::bswap_exp(arg.clone(), 64));
                    }
                    if name == "__builtin_convertvector" && args.len() == 2 {
                        if let Exp::Var(type_name) = &args[1] {
                            if let Some(info) = self.lookup_visible_typedef(type_name) {
                                let target_ft = info.full_type.clone();
                                return Ok(Exp::Cast(
                                    target_ft.to_ctype(),
                                    Some(target_ft),
                                    Box::new(args[0].clone()),
                                ));
                            }
                        }
                    }
                    if name == "__atomic_load_n" && args.len() == 2 {
                        return Ok(Self::ordered_atomic_builtin_exp(Exp::Unary(
                            UnaryOp::Deref,
                            Box::new(args[0].clone()),
                        )));
                    }
                    if name == "__atomic_store_n" && args.len() == 3 {
                        return Ok(Self::ordered_atomic_builtin_exp(Exp::Assign(
                            Box::new(Exp::Unary(UnaryOp::Deref, Box::new(args[0].clone()))),
                            Box::new(args[1].clone()),
                        )));
                    }
                    if name == "__atomic_exchange_n" && args.len() == 3 {
                        return Ok(Exp::AtomicExchange {
                            ptr: Box::new(args[0].clone()),
                            value: Box::new(args[1].clone()),
                        });
                    }
                    if name == "__atomic_compare_exchange_n" && args.len() == 6 {
                        return Ok(Exp::AtomicCompareExchange {
                            ptr: Box::new(args[0].clone()),
                            expected: Box::new(args[1].clone()),
                            desired: Box::new(args[2].clone()),
                        });
                    }
                    if matches!(
                        name.as_str(),
                        "__sync_bool_compare_and_swap" | "__sync_val_compare_and_swap"
                    ) && args.len() == 3
                    {
                        return Ok(Exp::AtomicCompareSwap {
                            ptr: Box::new(args[0].clone()),
                            expected: Box::new(args[1].clone()),
                            desired: Box::new(args[2].clone()),
                            return_old: name == "__sync_val_compare_and_swap",
                        });
                    }
                    if let Some(op) = Self::atomic_fetch_op(&name) {
                        let min_args = if name.starts_with("__sync_") { 2 } else { 3 };
                        if args.len() == min_args {
                            return Ok(Exp::AtomicFetch {
                                op,
                                ptr: Box::new(args[0].clone()),
                                arg: Box::new(args[1].clone()),
                                return_old: Self::atomic_fetch_returns_old(&name),
                            });
                        }
                    }
                    if name == "__builtin_choose_expr" && args.len() == 3 {
                        let condition = self
                            .eval_integer_constant_exp_with_layout(&args[0])
                            .ok_or_else(|| {
                                self.format_error(
                                    "__builtin_choose_expr requires an integer constant condition",
                                )
                            })?;
                        if condition != 0 {
                            return Ok(args[1].clone());
                        }
                        return Ok(args[2].clone());
                    }
                    if matches!(name.as_str(), "__builtin_unreachable" | "__builtin_trap") {
                        return Ok(Exp::Unreachable);
                    }
                    if self.lookup_value_type(&name).is_some() || name.starts_with("__builtin_") {
                        Ok(Exp::FunctionCall(name, args))
                    } else {
                        Ok(Exp::ImplicitFunctionCall(name, args))
                    }
                } else if let Some(val) = self.lookup_enum_constant(&name) {
                    // Enum constant — resolve to integer literal
                    Ok(Exp::Constant(val))
                } else if self.lookup_value_type(&name).is_none() {
                    match name.as_str() {
                        "true" => Ok(Exp::Constant(1)),
                        "false" => Ok(Exp::Constant(0)),
                        "nullptr" | "__nullptr" => Ok(Self::nullptr_expression()),
                        _ => Ok(Exp::Var(name)),
                    }
                } else {
                    Ok(Exp::Var(name))
                }
            }
            Some(Token::OpenParen) => {
                if self.pos + 1 < self.tokens.len() && self.tokens[self.pos + 1] == Token::OpenBrace
                {
                    return self.parse_statement_expression();
                }
                if let Some(exp) = self.parse_parenthesized_literal()? {
                    return Ok(exp);
                }
                self.advance()?;
                let exp = self.parse_expression()?;
                self.expect_token(Token::CloseParen)?;
                Ok(exp)
            }
            other => Err(self.format_error(&format!("expected expression, found {:?}", other))),
        }
    }

    pub(super) fn parse_parenthesized_literal(&mut self) -> ParseResult<Option<Exp>> {
        let save = self.pos;
        let mut scan = self.pos;
        let mut parens = 0;
        while matches!(self.tokens.get(scan), Some(Token::OpenParen)) {
            if matches!(self.tokens.get(scan + 1), Some(Token::OpenBrace)) {
                self.pos = save;
                return Ok(None);
            }
            parens += 1;
            scan += 1;
        }
        let Some(literal) = self.tokens.get(scan).cloned() else {
            self.pos = save;
            return Ok(None);
        };
        for offset in 1..=parens {
            if !matches!(self.tokens.get(scan + offset), Some(Token::CloseParen)) {
                self.pos = save;
                return Ok(None);
            }
        }
        self.pos = scan + parens + 1;
        let exp = match literal {
            Token::IntLiteral(val) if val >= i32::MIN as i64 && val <= i32::MAX as i64 => {
                Exp::Constant(val)
            }
            Token::IntLiteral(val) => Exp::LongConstant(val),
            Token::LongLiteral(val) => Exp::LongConstant(val),
            Token::Int128Literal(val) => Exp::Int128Constant(val),
            Token::UIntLiteral(val) if val > u32::MAX as i64 => Exp::ULongConstant(val),
            Token::UIntLiteral(val) => Exp::UIntConstant(val),
            Token::ULongLiteral(val) => Exp::ULongConstant(val),
            Token::UInt128Literal(val) => Exp::UInt128Constant(val),
            Token::DoubleLiteral(val) => Exp::DoubleConstant(val),
            Token::LongDoubleLiteral(val) => Exp::LongDoubleConstant(val),
            Token::ImaginaryIntLiteral(val) => Exp::ImaginaryIntConstant(val),
            Token::ImaginaryDoubleLiteral(val) => Exp::ImaginaryDoubleConstant(val),
            Token::CharLiteral(val) => Exp::Constant(val),
            _ => {
                self.pos = save;
                return Ok(None);
            }
        };
        Ok(Some(exp))
    }

    pub(super) fn parse_arg_list(&mut self) -> ParseResult<Vec<Exp>> {
        if self.at(&Token::CloseParen) {
            return Ok(Vec::new());
        }
        let mut args = vec![self.parse_assignment()?];
        while self.eat(&Token::Comma) {
            args.push(self.parse_assignment()?);
        }
        Ok(args)
    }
}
