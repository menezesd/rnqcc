//! TACKY lowering for initializer lists and static initializers, plus
//! per-function emission. Continuation of `impl TackyGen` (see mod.rs).

use super::*;

impl TackyGen {
    /// Flatten array initializer and emit CopyToOffset for each scalar value.
    /// `base_offset` is the byte offset from the start of the array.
    /// `elem_sizes`: byte size of each sub-array level.
    /// For `int[4][2][6]`: elem_sizes = [48, 24, 4] (size of [2][6], [6], int)
    pub(super) fn emit_initializer_list_at(
        &mut self,
        arr_name: &str,
        target_ft: &FullType,
        elems: &[Exp],
        index: &mut usize,
        base_offset: i64,
    ) -> TackyResult<()> {
        if *index >= elems.len() {
            return Ok(());
        }

        match target_ft {
            FullType::Array { elem, size } => {
                self.reject_flexible_array_struct_array_initializer(target_ft)?;
                if *index < elems.len()
                    && matches!(elems[*index], Exp::StringLiteral(_))
                    && Self::is_one_dimensional_char_array(target_ft)
                {
                    self.emit_initializer_value_at(
                        arr_name,
                        target_ft,
                        &elems[*index],
                        base_offset,
                    )?;
                    *index += 1;
                    return Ok(());
                }
                while *index < elems.len() && matches!(elems[*index], Exp::DesignatedInit(_, _)) {
                    self.emit_initializer_value_at(
                        arr_name,
                        target_ft,
                        &elems[*index],
                        base_offset,
                    )?;
                    *index += 1;
                }
                if *index >= elems.len() {
                    return Ok(());
                }
                let elem_size = elem.byte_size_with(&self.struct_defs) as i64;
                let array_len = if *size == 0 {
                    elems.len().saturating_sub(*index)
                } else {
                    *size
                };
                for i in 0..array_len {
                    if *index >= elems.len() {
                        break;
                    }
                    let elem_init = &elems[*index];
                    if elem.is_struct() && self.typeof_exp(elem_init) == elem.as_ref().clone() {
                        self.emit_initializer_value_at(
                            arr_name,
                            elem,
                            elem_init,
                            base_offset + i as i64 * elem_size,
                        )?;
                        *index += 1;
                        continue;
                    }
                    if (Self::static_aggregate_initializer(elem_init).is_some()
                        || matches!(elem_init, Exp::StringLiteral(_)))
                        && (elem.is_array() || elem.is_struct())
                    {
                        self.emit_initializer_value_at(
                            arr_name,
                            elem,
                            elem_init,
                            base_offset + i as i64 * elem_size,
                        )?;
                        *index += 1;
                    } else {
                        self.emit_initializer_list_at(
                            arr_name,
                            elem,
                            elems,
                            index,
                            base_offset + i as i64 * elem_size,
                        )?;
                    }
                }
            }
            FullType::Struct(tag) => {
                let def = self
                    .struct_defs
                    .get(tag)
                    .cloned()
                    .ok_or_else(|| format!("Undefined struct: {}", tag))?;
                let max_members = if def.is_union { 1 } else { def.members.len() };
                for mem in def.members.iter().take(max_members) {
                    if *index >= elems.len() {
                        break;
                    }
                    let elem_init = &elems[*index];
                    if mem.bit_width.is_some() {
                        let (val, val_type) = self.emit_exp(elem_init.clone())?;
                        let val_conv = self.convert_to(val, val_type, mem.member_type);
                        self.store_bit_field_to_absolute_offset(
                            arr_name.to_string(),
                            mem,
                            base_offset + mem.offset as i64,
                            val_conv,
                        )?;
                        *index += 1;
                        continue;
                    }
                    if mem.member_full_type.is_struct()
                        || ((Self::static_aggregate_initializer(elem_init).is_some()
                            || matches!(elem_init, Exp::StringLiteral(_)))
                            && (mem.member_full_type.is_array()
                                || mem.member_full_type.is_struct()))
                    {
                        self.emit_initializer_value_at(
                            arr_name,
                            &mem.member_full_type,
                            elem_init,
                            base_offset + mem.offset as i64,
                        )?;
                        *index += 1;
                    } else {
                        self.emit_initializer_list_at(
                            arr_name,
                            &mem.member_full_type,
                            elems,
                            index,
                            base_offset + mem.offset as i64,
                        )?;
                    }
                }
            }
            FullType::Scalar(_) | FullType::Pointer(_) | FullType::Vector { .. } => {
                self.emit_initializer_value_at(arr_name, target_ft, &elems[*index], base_offset)?;
                *index += 1;
            }
            FullType::Function { .. } => {
                return Err("function type is not an initializer target".to_string());
            }
        }
        Ok(())
    }

    pub(super) fn emit_struct_init_at(
        &mut self,
        arr_name: &str,
        init: &Exp,
        tag: &str,
        base_offset: i64,
    ) -> TackyResult<()> {
        if let Exp::ArrayInit(elems) = init {
            let def = self
                .struct_defs
                .get(tag)
                .cloned()
                .ok_or_else(|| format!("Undefined struct: {}", tag))?;
            let has_designators = elems
                .iter()
                .any(|elem| matches!(elem, Exp::DesignatedInit(_, _)));
            if !has_designators {
                let mut index = 0usize;
                return self.emit_initializer_list_at(
                    arr_name,
                    &FullType::Struct(tag.to_string()),
                    elems,
                    &mut index,
                    base_offset,
                );
            }
            // For unions, the compound init initializes the first member only.
            // If the first member is an array/struct, treat the whole init as its initializer.
            if def.is_union && !def.members.is_empty() && !has_designators {
                let mem = &def.members[0];
                let mem_offset = base_offset + mem.offset as i64;
                if mem.member_full_type.is_array() {
                    // For union with array first member, check for string literal init
                    if elems.len() == 1 {
                        if let Exp::StringLiteral(ref s) = elems[0] {
                            let bytes = c_string_bytes(s);
                            let chars_to_copy = std::cmp::min(bytes.len(), mem.size);
                            for (j, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                                let src = self.fresh_tmp(CType::Char);
                                self.emit(TackyInstr::Copy {
                                    src: TackyVal::Constant(byte as i64),
                                    dst: src.clone(),
                                });
                                self.emit(TackyInstr::CopyToOffset {
                                    src,
                                    dst_name: arr_name.to_string(),
                                    offset: mem_offset + j as i64,
                                });
                            }
                            return Ok(());
                        }
                    }
                    // For compound array init, pass the elements
                    let mem_elem_sizes =
                        Self::compute_elem_sizes(&mem.member_full_type, &self.struct_defs);
                    let inner_scalar = {
                        let mut t = &mem.member_full_type;
                        while let FullType::Array { elem: e, .. } = t {
                            t = e;
                        }
                        t.to_ctype()
                    };
                    self.emit_array_init_flat(
                        arr_name,
                        init,
                        inner_scalar,
                        mem_offset,
                        &mem_elem_sizes,
                    )?;
                    return Ok(());
                } else if mem.member_full_type.is_struct() {
                    if let FullType::Struct(ref inner_tag) = mem.member_full_type {
                        let member_init = if elems.len() == 1 { &elems[0] } else { init };
                        self.emit_struct_init_at(arr_name, member_init, inner_tag, mem_offset)?;
                        return Ok(());
                    }
                }
                // For scalar first member, just use the first element
            }
            let max_members = if def.is_union { 1 } else { def.members.len() };
            let mut positional_index = 0usize;
            for elem in elems {
                if let Exp::DesignatedInit(designators, value) = elem {
                    self.emit_designated_init_at(
                        arr_name,
                        &FullType::Struct(tag.to_string()),
                        designators,
                        value,
                        base_offset,
                    )?;
                    continue;
                }
                if positional_index >= max_members {
                    break;
                }
                let mem = &def.members[positional_index];
                positional_index += 1;
                let mem_offset = base_offset + mem.offset as i64;
                match elem {
                    Exp::ArrayInit(_) if mem.member_full_type.is_array() => {
                        let mem_elem_sizes =
                            Self::compute_elem_sizes(&mem.member_full_type, &self.struct_defs);
                        let inner_scalar = {
                            let mut t = &mem.member_full_type;
                            while let FullType::Array { elem: e, .. } = t {
                                t = e;
                            }
                            t.to_ctype()
                        };
                        self.emit_array_init_flat(
                            arr_name,
                            elem,
                            inner_scalar,
                            mem_offset,
                            &mem_elem_sizes,
                        )?;
                    }
                    Exp::ArrayInit(_) if mem.member_full_type.is_struct() => {
                        if let FullType::Struct(ref inner_tag) = mem.member_full_type {
                            self.emit_struct_init_at(arr_name, elem, inner_tag, mem_offset)?;
                        }
                    }
                    Exp::StringLiteral(s) if mem.member_full_type.is_array() => {
                        let bytes = c_string_bytes(s);
                        let chars_to_copy = std::cmp::min(bytes.len(), mem.size);
                        for (j, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                            let src = self.fresh_tmp(CType::Char);
                            self.emit(TackyInstr::Copy {
                                src: TackyVal::Constant(byte as i64),
                                dst: src.clone(),
                            });
                            self.emit(TackyInstr::CopyToOffset {
                                src,
                                dst_name: arr_name.to_string(),
                                offset: mem_offset + j as i64,
                            });
                        }
                    }
                    _ if mem.member_full_type.is_struct() => {
                        // Struct member initialized from a struct-valued expression
                        let struct_size = mem.member_full_type.byte_size_with(&self.struct_defs);
                        let (val, val_type) = self.emit_exp(elem.clone())?;
                        let src_addr = if val_type == CType::Pointer {
                            val
                        } else {
                            self.get_struct_addr(val)
                        };
                        let dst_addr = self.fresh_tmp(CType::Pointer);
                        self.emit(TackyInstr::GetAddress {
                            src: TackyVal::Var(arr_name.to_string()),
                            dst: dst_addr.clone(),
                        });
                        let member_addr = self.fresh_tmp(CType::Pointer);
                        if mem_offset > 0 {
                            self.emit(TackyInstr::Binary {
                                op: TackyBinaryOp::Add,
                                left: dst_addr,
                                right: TackyVal::Constant(mem_offset),
                                dst: member_addr.clone(),
                            });
                        } else {
                            self.emit(TackyInstr::Copy {
                                src: dst_addr,
                                dst: member_addr.clone(),
                            });
                        }
                        self.emit_struct_copy_ptr_to_ptr(src_addr, member_addr, struct_size);
                    }
                    _ => {
                        let (val, val_type) = self.emit_exp(elem.clone())?;
                        let val_conv = self.convert_to(val, val_type, mem.member_type);
                        if mem.bit_width.is_some() {
                            self.store_bit_field_to_offset(arr_name.to_string(), mem, val_conv)?;
                        } else {
                            self.emit(TackyInstr::CopyToOffset {
                                src: val_conv,
                                dst_name: arr_name.to_string(),
                                offset: mem_offset,
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn emit_array_init_flat(
        &mut self,
        arr_name: &str,
        init: &Exp,
        scalar_type: CType,
        base_offset: i64,
        elem_sizes: &[i64],
    ) -> TackyResult<()> {
        match init {
            Exp::ArrayInit(elems) => {
                let this_elem_size = if !elem_sizes.is_empty() {
                    elem_sizes[0]
                } else {
                    scalar_type.size() as i64
                };
                let inner_sizes = if elem_sizes.len() > 1 {
                    &elem_sizes[1..]
                } else {
                    &[]
                };
                let array_ft = FullType::Array {
                    elem: Box::new(if inner_sizes.is_empty() {
                        FullType::Scalar(scalar_type)
                    } else {
                        FullType::Array {
                            elem: Box::new(FullType::Scalar(scalar_type)),
                            size: (this_elem_size / inner_sizes[0]) as usize,
                        }
                    }),
                    size: elems.len(),
                };
                let mut positional_index = 0usize;
                for elem in elems {
                    if let Exp::DesignatedInit(designators, value) = elem {
                        self.emit_designated_init_at(
                            arr_name,
                            &array_ft,
                            designators,
                            value,
                            base_offset,
                        )?;
                        continue;
                    }
                    let elem_offset = base_offset + (positional_index as i64) * this_elem_size;
                    positional_index += 1;
                    match elem {
                        Exp::ArrayInit(_)
                            if inner_sizes.is_empty() && scalar_type == CType::Struct =>
                        {
                            // Compound initializer for struct/union element in array
                            // Find the struct tag for the array element type
                            let struct_tag =
                                self.array_struct_element_tag_at_offset(arr_name, base_offset)?;
                            self.emit_struct_init_at(arr_name, elem, &struct_tag, elem_offset)?;
                        }
                        _ if inner_sizes.is_empty()
                            && scalar_type == CType::Struct
                            && self.typeof_exp(elem).is_struct() =>
                        {
                            // Struct-valued expression in struct/union array
                            let arr_ft = self.get_full_type(arr_name);
                            let struct_tag = {
                                let mut t = &arr_ft;
                                while let FullType::Array { elem: e, .. } = t {
                                    t = e;
                                }
                                match t {
                                    FullType::Struct(tag) => tag.clone(),
                                    _ => return Err("Expected struct in array".to_string()),
                                }
                            };
                            let struct_size = self
                                .struct_defs
                                .get(&struct_tag)
                                .map(|d| d.size)
                                .unwrap_or(0);
                            let (val, val_type) = self.emit_exp(elem.clone())?;
                            let src_addr = if val_type == CType::Pointer {
                                val
                            } else {
                                self.get_struct_addr(val)
                            };
                            let dst_addr = self.fresh_tmp(CType::Pointer);
                            self.emit(TackyInstr::GetAddress {
                                src: TackyVal::Var(arr_name.to_string()),
                                dst: dst_addr.clone(),
                            });
                            let elem_addr = self.fresh_tmp(CType::Pointer);
                            if elem_offset > 0 {
                                self.emit(TackyInstr::Binary {
                                    op: TackyBinaryOp::Add,
                                    left: dst_addr,
                                    right: TackyVal::Constant(elem_offset),
                                    dst: elem_addr.clone(),
                                });
                            } else {
                                self.emit(TackyInstr::Copy {
                                    src: dst_addr,
                                    dst: elem_addr.clone(),
                                });
                            }
                            self.emit_struct_copy_ptr_to_ptr(src_addr, elem_addr, struct_size);
                        }
                        Exp::ArrayInit(_) => {
                            self.emit_array_init_flat(
                                arr_name,
                                elem,
                                scalar_type,
                                elem_offset,
                                inner_sizes,
                            )?;
                        }
                        Exp::StringLiteral(s) if scalar_type == CType::Pointer => {
                            // String literal in array of pointers context: decay to pointer
                            let (val, val_type) = self.emit_exp(elem.clone())?;
                            let val_conv = self.convert_to(val, val_type, scalar_type);
                            self.emit(TackyInstr::CopyToOffset {
                                src: val_conv,
                                dst_name: arr_name.to_string(),
                                offset: elem_offset,
                            });
                        }
                        Exp::StringLiteral(s) => {
                            // String literal as sub-element of char array compound init
                            let bytes = c_string_bytes(s);
                            let chars_to_copy = std::cmp::min(bytes.len(), this_elem_size as usize);
                            let char_type = if scalar_type == CType::UChar {
                                CType::UChar
                            } else {
                                CType::Char
                            };
                            for (j, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                                let src = self.fresh_tmp(char_type);
                                self.emit(TackyInstr::Copy {
                                    src: TackyVal::Constant(byte as i64),
                                    dst: src.clone(),
                                });
                                self.emit(TackyInstr::CopyToOffset {
                                    src,
                                    dst_name: arr_name.to_string(),
                                    offset: elem_offset + j as i64,
                                });
                            }
                        }
                        Exp::WideStringLiteral(_)
                        | Exp::Utf16StringLiteral(_)
                        | Exp::Utf32StringLiteral(_) => {
                            self.emit_prefixed_string_to_local_array(
                                arr_name,
                                elem,
                                scalar_type,
                                elem_offset,
                                this_elem_size as usize,
                            );
                        }
                        _ => {
                            let (val, val_type) = self.emit_exp(elem.clone())?;
                            let val_conv = self.convert_to(val, val_type, scalar_type);
                            self.emit(TackyInstr::CopyToOffset {
                                src: val_conv,
                                dst_name: arr_name.to_string(),
                                offset: elem_offset,
                            });
                        }
                    }
                }
            }
            Exp::StringLiteral(s) => {
                // String literal fills the array; use its own length, not per-element size
                let bytes = c_string_bytes(s);
                let chars_to_copy = bytes.len();
                let char_type = if scalar_type == CType::UChar {
                    CType::UChar
                } else {
                    CType::Char
                };
                for (i, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                    let src = self.fresh_tmp(char_type);
                    self.emit(TackyInstr::Copy {
                        src: TackyVal::Constant(byte as i64),
                        dst: src.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src,
                        dst_name: arr_name.to_string(),
                        offset: base_offset + i as i64,
                    });
                }
            }
            Exp::WideStringLiteral(_) | Exp::Utf16StringLiteral(_) | Exp::Utf32StringLiteral(_) => {
                self.emit_prefixed_string_to_local_array(
                    arr_name,
                    init,
                    scalar_type,
                    base_offset,
                    self.local_array_remaining_bytes(arr_name, base_offset),
                );
            }
            _ => {
                let (val, val_type) = self.emit_exp(init.clone())?;
                let val_conv = self.convert_to(val, val_type, scalar_type);
                self.emit(TackyInstr::CopyToOffset {
                    src: val_conv,
                    dst_name: arr_name.to_string(),
                    offset: base_offset,
                });
            }
        }
        Ok(())
    }

    pub(super) fn emit_string_units_to_local_array(
        &mut self,
        arr_name: &str,
        units: impl IntoIterator<Item = i64>,
        scalar_type: CType,
        base_offset: i64,
        max_bytes: usize,
    ) {
        let elem_size = scalar_type.size() as usize;
        if elem_size == 0 {
            return;
        }
        let max_units = max_bytes / elem_size;
        for (i, unit) in units.into_iter().take(max_units).enumerate() {
            let src = self.fresh_tmp(scalar_type);
            self.emit(TackyInstr::Copy {
                src: TackyVal::Constant(unit),
                dst: src.clone(),
            });
            self.emit(TackyInstr::CopyToOffset {
                src,
                dst_name: arr_name.to_string(),
                offset: base_offset + (i * elem_size) as i64,
            });
        }
    }

    pub(super) fn emit_prefixed_string_to_local_array(
        &mut self,
        arr_name: &str,
        init: &Exp,
        scalar_type: CType,
        base_offset: i64,
        max_bytes: usize,
    ) {
        match init {
            Exp::WideStringLiteral(s) | Exp::Utf32StringLiteral(s) => {
                self.emit_string_units_to_local_array(
                    arr_name,
                    s.chars().map(|ch| ch as i64),
                    scalar_type,
                    base_offset,
                    max_bytes,
                );
            }
            Exp::Utf16StringLiteral(s) => {
                self.emit_string_units_to_local_array(
                    arr_name,
                    s.encode_utf16().map(i64::from),
                    scalar_type,
                    base_offset,
                    max_bytes,
                );
            }
            _ => {}
        }
    }

    pub(super) fn local_array_remaining_bytes(&self, arr_name: &str, base_offset: i64) -> usize {
        let total = self
            .array_sizes
            .get(arr_name)
            .copied()
            .unwrap_or(usize::MAX);
        let offset = usize::try_from(base_offset).unwrap_or(usize::MAX);
        total.saturating_sub(offset)
    }

    pub(super) fn direct_array_struct_elem(ft: &FullType) -> Option<DirectArrayStructElem<'_>> {
        match ft {
            FullType::Array { elem, size } => match elem.as_ref() {
                FullType::Struct(tag) => Some(DirectArrayStructElem {
                    tag: tag.as_str(),
                    array_len: *size,
                }),
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) fn emit_struct_member_initializer(
        &mut self,
        arr_name: &str,
        mem: &StructMember,
        value: &Exp,
        offset: i64,
    ) -> TackyResult<()> {
        if mem.bit_width.is_some() {
            let (val, val_type) = self.emit_exp(value.clone())?;
            let val_conv = self.convert_to(val, val_type, mem.member_type);
            self.store_bit_field_to_absolute_offset(arr_name.to_string(), mem, offset, val_conv)?;
            return Ok(());
        }

        self.emit_initializer_value_at(arr_name, &mem.member_full_type, value, offset)
    }

    pub(super) fn reject_flexible_array_struct_array_initializer(
        &self,
        target_ft: &FullType,
    ) -> TackyResult<()> {
        let FullType::Array { elem, .. } = target_ft else {
            return Ok(());
        };
        let FullType::Struct(tag) = elem.as_ref() else {
            return Ok(());
        };
        let has_flexible_member = self
            .struct_defs
            .get(tag)
            .is_some_and(|def| def.members.iter().any(|member| member.flexible_array));
        if has_flexible_member {
            Err("initialization of flexible array member".to_string())
        } else {
            Ok(())
        }
    }

    pub(super) fn emit_struct_array_init_flat(
        &mut self,
        arr_name: &str,
        init: &Exp,
        tag: &str,
        array_len: usize,
        base_offset: i64,
    ) -> TackyResult<()> {
        let Exp::ArrayInit(elems) = init else {
            return self.emit_initializer_value_at(
                arr_name,
                &FullType::Array {
                    elem: Box::new(FullType::Struct(tag.to_string())),
                    size: array_len,
                },
                init,
                base_offset,
            );
        };

        let def = self
            .struct_defs
            .get(tag)
            .cloned()
            .ok_or_else(|| format!("Undefined struct: {}", tag))?;
        if def.members.iter().any(|member| member.flexible_array) {
            return Err("initialization of flexible array member".to_string());
        }
        let struct_size = def.size as i64;
        let max_members = if def.is_union { 1 } else { def.members.len() };
        let mut elem_index = 0usize;
        let mut member_index = 0usize;

        for elem in elems {
            if elem_index >= array_len {
                break;
            }
            let elem_offset = base_offset + elem_index as i64 * struct_size;
            match elem {
                Exp::DesignatedInit(designators, value) => {
                    self.emit_designated_init_at(
                        arr_name,
                        &FullType::Array {
                            elem: Box::new(FullType::Struct(tag.to_string())),
                            size: array_len,
                        },
                        designators,
                        value,
                        base_offset,
                    )?;
                }
                Exp::ArrayInit(_) => {
                    self.emit_struct_init_at(arr_name, elem, tag, elem_offset)?;
                    elem_index += 1;
                    member_index = 0;
                }
                _ => {
                    let elem_ft = FullType::Struct(tag.to_string());
                    if self.typeof_exp(elem) == elem_ft {
                        self.emit_initializer_value_at(arr_name, &elem_ft, elem, elem_offset)?;
                        elem_index += 1;
                        member_index = 0;
                        continue;
                    }
                    if member_index >= max_members {
                        elem_index += 1;
                        member_index = 0;
                    }
                    if elem_index >= array_len || member_index >= max_members {
                        break;
                    }
                    let elem_offset = base_offset + elem_index as i64 * struct_size;
                    let mem = &def.members[member_index];
                    self.emit_struct_member_initializer(
                        arr_name,
                        mem,
                        elem,
                        elem_offset + mem.offset as i64,
                    )?;
                    member_index += 1;
                    if member_index >= max_members {
                        elem_index += 1;
                        member_index = 0;
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn array_struct_element_tag_at_offset(
        &self,
        arr_name: &str,
        base_offset: i64,
    ) -> TackyResult<String> {
        let arr_ft = self.get_full_type(arr_name);
        let mut t = &arr_ft;
        while let FullType::Array { elem: e, .. } = t {
            t = e;
        }
        if let FullType::Struct(tag) = t {
            if let Some(def) = self.struct_defs.get(tag) {
                for mem in &def.members {
                    let start = mem.offset as i64;
                    let end = start + mem.member_full_type.byte_size_with(&self.struct_defs) as i64;
                    if base_offset >= start && base_offset < end {
                        let mut mt = &mem.member_full_type;
                        while let FullType::Array { elem, .. } = mt {
                            mt = elem;
                        }
                        if let FullType::Struct(member_tag) = mt {
                            return Ok(member_tag.clone());
                        }
                    }
                }
            }
            return Ok(tag.clone());
        }
        Err("Expected struct in array".to_string())
    }

    /// Compute element sizes for each array dimension.
    /// For `int[4][2][6]`: returns [48, 24, 4] (sizes of [2][6], [6], int)
    pub(super) fn compute_elem_sizes(
        ft: &FullType,
        struct_defs: &IndexMap<String, StructDef>,
    ) -> Vec<i64> {
        let mut sizes = Vec::new();
        let mut t = ft;
        while let FullType::Array { elem, .. } = t {
            sizes.push(elem.byte_size_with(struct_defs) as i64);
            t = elem;
        }
        sizes
    }

    pub(super) fn eval_designator_index(exp: &Exp) -> Option<i64> {
        match exp {
            Exp::Constant(c)
            | Exp::LongConstant(c)
            | Exp::UIntConstant(c)
            | Exp::ULongConstant(c) => Some(*c),
            Exp::Unary(UnaryOp::Negate, inner) => Self::eval_designator_index(inner).map(|v| -v),
            Exp::Unary(UnaryOp::Complement, inner) => {
                Self::eval_designator_index(inner).map(|v| !v)
            }
            Exp::Unary(UnaryOp::LogicalNot, inner) => {
                Self::eval_designator_index(inner).map(|v| (v == 0) as i64)
            }
            Exp::Binary(op, left, right) => {
                let left = Self::eval_designator_index(left)?;
                let right = Self::eval_designator_index(right)?;
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
            _ => None,
        }
    }

    pub(super) fn static_designated_initializer_target(
        &self,
        base_ft: &FullType,
        designators: &[Designator],
        value: &Exp,
        base_offset: usize,
    ) -> TackyResult<(FullType, usize, Exp)> {
        let Some((designator, rest)) = designators.split_first() else {
            return Ok((base_ft.clone(), base_offset, value.clone()));
        };

        match (designator, base_ft) {
            (Designator::Index(index), FullType::Array { elem, size }) => {
                let index = Self::eval_designator_index(index)
                    .ok_or_else(|| "array designator index must be constant".to_string())?;
                if index < 0 || index as usize >= *size {
                    return Err(format!("array designator index {} out of bounds", index));
                }
                self.static_designated_initializer_target(
                    elem,
                    rest,
                    value,
                    base_offset + index as usize * elem.byte_size_with(&self.struct_defs),
                )
            }
            (Designator::IndexRange(_, _), FullType::Array { .. }) => {
                Err("array range designator is only valid inside initializer lists".to_string())
            }
            (Designator::Field(name), FullType::Struct(tag)) => {
                let def = self
                    .struct_defs
                    .get(tag)
                    .ok_or_else(|| format!("Undefined struct: {}", tag))?;
                let mem = def
                    .find_member(name)
                    .ok_or_else(|| format!("struct '{}' has no member '{}'", tag, name))?;
                self.static_designated_initializer_target(
                    &mem.member_full_type,
                    rest,
                    value,
                    base_offset + mem.offset,
                )
            }
            _ => Err(format!(
                "invalid initializer designator for type {:?}",
                base_ft
            )),
        }
    }

    pub(super) fn statement_contains_label(stmt: &Statement) -> bool {
        match stmt {
            Statement::Label(_, _) | Statement::Case { .. } | Statement::Default { .. } => true,
            Statement::Block(block) => block.iter().any(|item| {
                matches!(item, BlockItem::Statement(stmt) if Self::statement_contains_label(stmt))
            }),
            Statement::If(_, then_stmt, else_stmt) => {
                Self::statement_contains_label(then_stmt)
                    || else_stmt
                        .as_deref()
                        .is_some_and(Self::statement_contains_label)
            }
            Statement::While { body, .. }
            | Statement::DoWhile { body, .. }
            | Statement::For { body, .. }
            | Statement::Switch { body, .. } => Self::statement_contains_label(body),
            _ => false,
        }
    }

    pub(super) fn prune_unreachable_prefix_to_label(stmt: Statement) -> Option<Statement> {
        match stmt {
            Statement::Label(_, _) | Statement::Case { .. } | Statement::Default { .. } => {
                Some(stmt)
            }
            Statement::Block(block) => {
                let mut out = Vec::new();
                let mut found = false;
                for item in block {
                    if found {
                        out.push(item);
                        continue;
                    }
                    if let BlockItem::Statement(stmt) = item {
                        if let Some(pruned) = Self::prune_unreachable_prefix_to_label(stmt) {
                            out.push(BlockItem::Statement(pruned));
                            found = true;
                        }
                    }
                }
                found.then_some(Statement::Block(out))
            }
            Statement::If(_, then_stmt, else_stmt) => {
                Self::prune_unreachable_prefix_to_label(*then_stmt).or_else(|| {
                    else_stmt
                        .and_then(|else_stmt| Self::prune_unreachable_prefix_to_label(*else_stmt))
                })
            }
            Statement::While { body, .. }
            | Statement::DoWhile { body, .. }
            | Statement::For { body, .. }
            | Statement::Switch { body, .. } => Self::prune_unreachable_prefix_to_label(*body),
            _ => None,
        }
    }

    pub(super) fn array_scalar_type(ft: &FullType) -> CType {
        let mut t = ft;
        while let FullType::Array { elem, .. } = t {
            t = elem;
        }
        t.to_ctype()
    }

    pub(super) fn emit_initializer_value_at(
        &mut self,
        arr_name: &str,
        target_ft: &FullType,
        value: &Exp,
        offset: i64,
    ) -> TackyResult<()> {
        if target_ft.is_complex() {
            let (elem_type, elem_size) = match target_ft {
                FullType::Vector { elem, .. } => {
                    (elem.to_ctype(), elem.byte_size_with(&self.struct_defs))
                }
                _ => (CType::Double, 8),
            };
            match value {
                Exp::ArrayInit(elems) => {
                    if let Some(first) = elems.first() {
                        if elems.len() == 1 && self.typeof_exp(first).is_complex() {
                            let (val, val_type) = self.emit_exp(first.clone())?;
                            let val_ft = self.val_full_type(&val);
                            self.emit_complex_value_to_offset(
                                arr_name, target_ft, val, val_type, val_ft, offset,
                            )?;
                            return Ok(());
                        }
                        let (val, val_type) = self.emit_exp(first.clone())?;
                        let real = self.convert_to(val, val_type, elem_type);
                        self.emit(TackyInstr::CopyToOffset {
                            src: real,
                            dst_name: arr_name.to_string(),
                            offset,
                        });
                    }
                    if let Some(second) = elems.get(1) {
                        let (val, val_type) = self.emit_exp(second.clone())?;
                        let imag = self.convert_to(val, val_type, elem_type);
                        self.emit(TackyInstr::CopyToOffset {
                            src: imag,
                            dst_name: arr_name.to_string(),
                            offset: offset + elem_size as i64,
                        });
                    }
                }
                _ => {
                    let (val, val_type) = self.emit_exp(value.clone())?;
                    let real = self.convert_to(val, val_type, elem_type);
                    self.emit(TackyInstr::CopyToOffset {
                        src: real,
                        dst_name: arr_name.to_string(),
                        offset,
                    });
                }
            }
            return Ok(());
        }
        match (target_ft, value) {
            (_, Exp::DesignatedInit(designators, inner)) => {
                self.emit_designated_init_at(arr_name, target_ft, designators, inner, offset)?;
            }
            (FullType::Array { .. }, Exp::ArrayInit(elems)) => {
                let mut index = 0usize;
                self.emit_initializer_list_at(arr_name, target_ft, elems, &mut index, offset)?;
            }
            (FullType::Array { .. }, Exp::StringLiteral(_)) => {
                let elem_sizes = Self::compute_elem_sizes(target_ft, &self.struct_defs);
                self.emit_array_init_flat(
                    arr_name,
                    value,
                    Self::array_scalar_type(target_ft),
                    offset,
                    &elem_sizes,
                )?;
            }
            (FullType::Struct(tag), Exp::ArrayInit(_)) => {
                self.emit_struct_init_at(arr_name, value, tag, offset)?;
            }
            (FullType::Struct(tag), _) => {
                if let Some(first_member) = self
                    .struct_defs
                    .get(tag)
                    .filter(|def| def.is_union)
                    .and_then(|def| def.members.first())
                    .cloned()
                {
                    return self.emit_struct_member_initializer(
                        arr_name,
                        &first_member,
                        value,
                        offset + first_member.offset as i64,
                    );
                }
                let struct_size = target_ft.byte_size_with(&self.struct_defs);
                let (val, val_type) = self.emit_exp(value.clone())?;
                let src_addr = if val_type == CType::Pointer {
                    val
                } else {
                    self.get_struct_addr(val)
                };
                let dst_addr = self.fresh_tmp(CType::Pointer);
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(arr_name.to_string()),
                    dst: dst_addr.clone(),
                });
                let target_addr = self.fresh_tmp(CType::Pointer);
                if offset > 0 {
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: dst_addr,
                        right: TackyVal::Constant(offset),
                        dst: target_addr.clone(),
                    });
                } else {
                    self.emit(TackyInstr::Copy {
                        src: dst_addr,
                        dst: target_addr.clone(),
                    });
                }
                self.emit_struct_copy_ptr_to_ptr(src_addr, target_addr, struct_size);
            }
            _ => {
                self.assert_pointer_initializer_assignable(target_ft, value)?;
                let target_type = target_ft.to_ctype();
                let (val, val_type) = self.emit_exp(value.clone())?;
                let val_conv = self.convert_to(val, val_type, target_type);
                self.emit(TackyInstr::CopyToOffset {
                    src: val_conv,
                    dst_name: arr_name.to_string(),
                    offset,
                });
            }
        }
        Ok(())
    }

    pub(super) fn emit_designated_init_at(
        &mut self,
        arr_name: &str,
        base_ft: &FullType,
        designators: &[Designator],
        value: &Exp,
        base_offset: i64,
    ) -> TackyResult<()> {
        if designators.is_empty() {
            return self.emit_initializer_value_at(arr_name, base_ft, value, base_offset);
        }

        match (&designators[0], base_ft) {
            (Designator::Index(index), FullType::Array { elem, size: _ }) => {
                let index = Self::eval_designator_index(index)
                    .ok_or_else(|| "array designator index must be constant".to_string())?;
                if index < 0 {
                    return Err(format!("array designator index {} out of bounds", index));
                }
                let elem_size = elem.byte_size_with(&self.struct_defs) as i64;
                self.emit_designated_init_at(
                    arr_name,
                    elem,
                    &designators[1..],
                    value,
                    base_offset + index * elem_size,
                )?;
            }
            (Designator::IndexRange(start, end), FullType::Array { elem, size }) => {
                let start = Self::eval_designator_index(start)
                    .ok_or_else(|| "array designator range start must be constant".to_string())?;
                let end = Self::eval_designator_index(end)
                    .ok_or_else(|| "array designator range end must be constant".to_string())?;
                if start < 0 || end < start || end as usize >= *size {
                    return Err(format!(
                        "array designator range {}...{} out of bounds",
                        start, end
                    ));
                }
                let elem_size = elem.byte_size_with(&self.struct_defs) as i64;
                for index in start..=end {
                    self.emit_designated_init_at(
                        arr_name,
                        elem,
                        &designators[1..],
                        value,
                        base_offset + index * elem_size,
                    )?;
                }
            }
            (Designator::Field(name), FullType::Struct(tag)) => {
                let def = self
                    .struct_defs
                    .get(tag)
                    .ok_or_else(|| format!("Undefined struct: {}", tag))?;
                let mem = def
                    .find_member(name)
                    .ok_or_else(|| format!("struct '{}' has no member '{}'", tag, name))?
                    .clone();
                let member_offset = base_offset + mem.offset as i64;
                if designators.len() == 1 {
                    self.emit_struct_member_initializer(arr_name, &mem, value, member_offset)?;
                } else {
                    self.emit_designated_init_at(
                        arr_name,
                        &mem.member_full_type,
                        &designators[1..],
                        value,
                        member_offset,
                    )?;
                }
            }
            _ => {
                return Err(format!(
                    "invalid initializer designator for type {:?}",
                    base_ft
                ))
            }
        }
        Ok(())
    }

    pub(super) fn put_static_initializer(
        &mut self,
        builder: &mut StaticInitBuilder,
        base_ft: &FullType,
        init: &Exp,
        base_offset: usize,
    ) -> TackyResult<()> {
        if base_ft.is_complex() {
            let (elem_type, elem_size) = match base_ft {
                FullType::Vector { elem, .. } => {
                    (elem.to_ctype(), elem.byte_size_with(&self.struct_defs))
                }
                _ => (CType::Double, 8),
            };
            match init {
                Exp::ArrayInit(elems) => {
                    if let Some(first) = elems.first() {
                        let v = self.eval_static_constant_init(&Some(first.clone()))?;
                        let cv = convert_init_value(v, elem_type);
                        builder.put(base_offset, make_static_init(cv, elem_type))?;
                    }
                    if let Some(second) = elems.get(1) {
                        let v = self.eval_static_constant_init(&Some(second.clone()))?;
                        let cv = convert_init_value(v, elem_type);
                        builder.put(base_offset + elem_size, make_static_init(cv, elem_type))?;
                    }
                }
                _ => {
                    let value = self
                        .eval_static_complex_constant_init(init)
                        .ok_or_else(|| "Static complex initializer must be constant".to_string())?;
                    let cv = convert_init_value(value.real, elem_type);
                    builder.put(base_offset, make_static_init(cv, elem_type))?;
                    let cv = convert_init_value(value.imag, elem_type);
                    builder.put(base_offset + elem_size, make_static_init(cv, elem_type))?;
                }
            }
            return Ok(());
        }
        if let Exp::DesignatedInit(designators, value) = init {
            if let (FullType::Struct(tag), [Designator::Field(name)]) =
                (base_ft, designators.as_slice())
            {
                let def = self
                    .struct_defs
                    .get(tag)
                    .ok_or_else(|| format!("Undefined struct: {}", tag))?;
                let mem = def
                    .find_member(name)
                    .ok_or_else(|| format!("struct '{}' has no member '{}'", tag, name))?;
                if let Some(width) = mem.bit_width {
                    let raw = self.eval_static_scalar_init_for_type(
                        &Some(value.as_ref().clone()),
                        mem.member_type,
                    )?;
                    let converted = convert_init_value(raw, mem.member_type);
                    builder.put_bit_field(
                        base_offset + mem.offset,
                        mem.member_type,
                        converted,
                        mem.bit_offset,
                        width,
                    )?;
                    return Ok(());
                }
            }
            if let (
                Some((Designator::IndexRange(start, end), rest)),
                FullType::Array { elem, size },
            ) = (designators.split_first(), base_ft)
            {
                let start = Self::eval_designator_index(start)
                    .ok_or_else(|| "array designator range start must be constant".to_string())?;
                let end = Self::eval_designator_index(end)
                    .ok_or_else(|| "array designator range end must be constant".to_string())?;
                if start < 0 || end < start || end as usize >= *size {
                    return Err(format!(
                        "array designator range {}...{} out of bounds",
                        start, end
                    ));
                }
                let value = if rest.is_empty() {
                    value.as_ref().clone()
                } else {
                    Exp::DesignatedInit(rest.to_vec(), value.clone())
                };
                let elem_size = elem.byte_size_with(&self.struct_defs);
                for index in start as usize..=end as usize {
                    self.put_static_initializer(
                        builder,
                        elem,
                        &value,
                        base_offset + index * elem_size,
                    )?;
                }
                return Ok(());
            }
            let (target_ft, target_offset, target_value) = self
                .static_designated_initializer_target(base_ft, designators, value, base_offset)?;
            return self.put_static_initializer(builder, &target_ft, &target_value, target_offset);
        }
        if let Exp::Cast(_, _, inner) = init {
            if let Exp::ArrayInit(elems) = inner.as_ref() {
                if base_ft.is_array() || base_ft.is_struct() || base_ft.is_vector() {
                    return self.put_static_initializer(builder, base_ft, inner, base_offset);
                }
                if let [value] = elems.as_slice() {
                    return self.put_static_initializer(builder, base_ft, value, base_offset);
                }
            }
        }
        match (base_ft, init) {
            (FullType::Array { .. }, Exp::ArrayInit(_))
                if Self::is_one_dimensional_char_array(base_ft)
                    && Self::string_array_initializer(init).is_some() =>
            {
                let s = Self::string_array_initializer(init)
                    .ok_or_else(|| "expected string array initializer".to_string())?;
                self.put_static_initializer(
                    builder,
                    base_ft,
                    &Exp::StringLiteral(s.clone()),
                    base_offset,
                )?;
            }
            (FullType::Array { size: 0, .. }, Exp::StringLiteral(s))
                if Self::is_one_dimensional_char_array(base_ft) =>
            {
                builder.put(base_offset, StaticInit::StringInit(s.clone(), false))?;
            }
            (FullType::Array { elem, size }, Exp::ArrayInit(elems)) => {
                self.reject_flexible_array_struct_array_initializer(base_ft)?;
                if !elems
                    .iter()
                    .any(|elem| matches!(elem, Exp::DesignatedInit(_, _)))
                {
                    let mut index = 0usize;
                    self.put_static_initializer_list(
                        builder,
                        base_ft,
                        elems,
                        &mut index,
                        base_offset,
                    )?;
                    return Ok(());
                }
                let elem_size = elem.byte_size_with(&self.struct_defs);
                let mut positional_index = 0usize;
                for elem_init in elems {
                    if let Exp::DesignatedInit(designators, value) = elem_init {
                        let (designator, rest) = designators
                            .split_first()
                            .ok_or_else(|| "empty initializer designator".to_string())?;
                        let value = if rest.is_empty() {
                            value.as_ref().clone()
                        } else {
                            Exp::DesignatedInit(rest.to_vec(), value.clone())
                        };
                        match designator {
                            Designator::Index(index) => {
                                let index =
                                    Self::eval_designator_index(index).ok_or_else(|| {
                                        "array designator index must be constant".to_string()
                                    })?;
                                if index < 0 || index as usize >= *size {
                                    return Err(format!(
                                        "array designator index {} out of bounds",
                                        index
                                    ));
                                }
                                let index = index as usize;
                                self.put_static_initializer(
                                    builder,
                                    elem,
                                    &value,
                                    base_offset + index * elem_size,
                                )?;
                                positional_index = index + 1;
                            }
                            Designator::IndexRange(start, end) => {
                                let start =
                                    Self::eval_designator_index(start).ok_or_else(|| {
                                        "array designator range start must be constant".to_string()
                                    })?;
                                let end = Self::eval_designator_index(end).ok_or_else(|| {
                                    "array designator range end must be constant".to_string()
                                })?;
                                if start < 0 || end < start || end as usize >= *size {
                                    return Err(format!(
                                        "array designator range {}...{} out of bounds",
                                        start, end
                                    ));
                                }
                                for index in start as usize..=end as usize {
                                    self.put_static_initializer(
                                        builder,
                                        elem,
                                        &value,
                                        base_offset + index * elem_size,
                                    )?;
                                }
                                positional_index = end as usize + 1;
                            }
                            Designator::Field(_) => {
                                return Err("invalid array initializer designator".to_string());
                            }
                        }
                    } else {
                        let index = positional_index;
                        positional_index += 1;
                        if index >= *size {
                            break;
                        }
                        self.put_static_initializer(
                            builder,
                            elem,
                            elem_init,
                            base_offset + index * elem_size,
                        )?;
                    }
                }
            }
            (FullType::Array { elem, size }, Exp::WideStringLiteral(s))
                if !elem.to_ctype().is_char() =>
            {
                let elem_size = elem.byte_size_with(&self.struct_defs);
                for (index, ch) in s.chars().take(*size).enumerate() {
                    self.put_static_initializer(
                        builder,
                        elem,
                        &Exp::Constant(ch as i64),
                        base_offset + index * elem_size,
                    )?;
                }
            }
            (FullType::Array { elem, size }, Exp::Utf16StringLiteral(s))
                if !elem.to_ctype().is_char() =>
            {
                let elem_size = elem.byte_size_with(&self.struct_defs);
                for (index, unit) in s.encode_utf16().take(*size).enumerate() {
                    self.put_static_initializer(
                        builder,
                        elem,
                        &Exp::Constant(i64::from(unit)),
                        base_offset + index * elem_size,
                    )?;
                }
            }
            (FullType::Array { elem, size }, Exp::Utf32StringLiteral(s))
                if !elem.to_ctype().is_char() =>
            {
                let elem_size = elem.byte_size_with(&self.struct_defs);
                for (index, ch) in s.chars().take(*size).enumerate() {
                    self.put_static_initializer(
                        builder,
                        elem,
                        &Exp::Constant(ch as i64),
                        base_offset + index * elem_size,
                    )?;
                }
            }
            (FullType::Array { elem, size }, Exp::StringLiteral(s)) => {
                let total_bytes = elem.byte_size_with(&self.struct_defs) * size;
                let string_bytes = c_string_byte_len(s);
                let null_terminated = string_bytes < total_bytes;
                let str_to_write = if string_bytes <= total_bytes {
                    s.clone()
                } else {
                    c_string_truncate_bytes(s, total_bytes)
                };
                builder.put(
                    base_offset,
                    StaticInit::StringInit(str_to_write, null_terminated),
                )?;
            }
            (FullType::Struct(tag), Exp::ArrayInit(elems)) => {
                let def = self
                    .struct_defs
                    .get(tag)
                    .cloned()
                    .ok_or_else(|| format!("Undefined struct: {}", tag))?;
                if def.is_union && elems.len() == 1 && matches!(elems[0], Exp::ArrayInit(_)) {
                    let Exp::ArrayInit(inner_elems) = &elems[0] else {
                        return Err("internal error: expected nested array initializer".to_string());
                    };
                    let mut index = 0usize;
                    for mem in &def.members {
                        if index >= inner_elems.len() {
                            break;
                        }
                        self.put_static_initializer_list(
                            builder,
                            &mem.member_full_type,
                            inner_elems,
                            &mut index,
                            base_offset + mem.offset,
                        )?;
                    }
                    return Ok(());
                }
                if !elems
                    .iter()
                    .any(|elem| matches!(elem, Exp::DesignatedInit(_, _)))
                {
                    let mut index = 0usize;
                    self.put_static_initializer_list(
                        builder,
                        base_ft,
                        elems,
                        &mut index,
                        base_offset,
                    )?;
                    return Ok(());
                }
                let max_members = if def.is_union { 1 } else { def.members.len() };
                let mut positional_index = 0usize;
                for elem_init in elems {
                    let (member, value) = if let Exp::DesignatedInit(designators, value) = elem_init
                    {
                        let (Designator::Field(name), rest) = designators
                            .split_first()
                            .ok_or_else(|| "empty initializer designator".to_string())?
                        else {
                            return Err("invalid struct initializer designator".to_string());
                        };
                        let member = def
                            .find_member(name)
                            .ok_or_else(|| format!("struct '{}' has no member '{}'", tag, name))?
                            .clone();
                        let value = if rest.is_empty() {
                            value.as_ref().clone()
                        } else {
                            Exp::DesignatedInit(rest.to_vec(), value.clone())
                        };
                        (member, value)
                    } else {
                        if positional_index >= max_members {
                            break;
                        }
                        let member = def.members[positional_index].clone();
                        positional_index += 1;
                        (member, elem_init.clone())
                    };
                    if let Some(width) = member.bit_width {
                        let raw = self.eval_static_scalar_init_for_type(
                            &Some(value.clone()),
                            member.member_type,
                        )?;
                        let converted = convert_init_value(raw, member.member_type);
                        builder.put_bit_field(
                            base_offset + member.offset,
                            member.member_type,
                            converted,
                            member.bit_offset,
                            width,
                        )?;
                        continue;
                    }
                    self.put_static_initializer(
                        builder,
                        &member.member_full_type,
                        &value,
                        base_offset + member.offset,
                    )?;
                }
            }
            (FullType::Pointer(_), Exp::StringLiteral(s)) => {
                self.assert_pointer_initializer_assignable(base_ft, init)?;
                let label = self.make_string_constant(s);
                builder.put(base_offset, StaticInit::PointerInit(label))?;
            }
            (FullType::Pointer(_), Exp::WideStringLiteral(s)) => {
                self.assert_pointer_initializer_assignable(base_ft, init)?;
                let label = self.make_raw_string_constant(
                    wide_string_bytes_with_null(s),
                    FullType::Array {
                        elem: Box::new(FullType::Scalar(CType::Int)),
                        size: s.chars().count() + 1,
                    },
                    FullType::Scalar(CType::Int).alignment(),
                );
                builder.put(base_offset, StaticInit::PointerInit(label))?;
            }
            (FullType::Pointer(_), Exp::Utf16StringLiteral(s)) => {
                self.assert_pointer_initializer_assignable(base_ft, init)?;
                let label = self.make_raw_string_constant(
                    utf16_string_bytes_with_null(s),
                    FullType::Array {
                        elem: Box::new(FullType::Scalar(CType::UShort)),
                        size: s.encode_utf16().count() + 1,
                    },
                    FullType::Scalar(CType::UShort).alignment(),
                );
                builder.put(base_offset, StaticInit::PointerInit(label))?;
            }
            (FullType::Pointer(_), Exp::Utf32StringLiteral(s)) => {
                self.assert_pointer_initializer_assignable(base_ft, init)?;
                let label = self.make_raw_string_constant(
                    utf32_string_bytes_with_null(s),
                    FullType::Array {
                        elem: Box::new(FullType::Scalar(CType::UInt)),
                        size: s.chars().count() + 1,
                    },
                    FullType::Scalar(CType::UInt).alignment(),
                );
                builder.put(base_offset, StaticInit::PointerInit(label))?;
            }
            (FullType::Vector { elem, lanes, .. }, Exp::ArrayInit(elems)) => {
                let elem_size = elem.byte_size_with(&self.struct_defs);
                for (index, elem_init) in elems.iter().take(*lanes).enumerate() {
                    self.put_static_initializer(
                        builder,
                        elem,
                        elem_init,
                        base_offset + index * elem_size,
                    )?;
                }
            }
            (FullType::Pointer(_), _) => {
                self.assert_pointer_initializer_assignable(base_ft, init)?;
                if let Some(ptr_init) = self.static_pointer_initializer(init) {
                    builder.put(base_offset, ptr_init)?;
                } else {
                    let v = self.eval_static_constant_init(&Some(init.clone()))?;
                    let cv = convert_init_value(v, CType::Pointer);
                    builder.put(base_offset, make_static_init(cv, CType::Pointer))?;
                }
            }
            (FullType::Scalar(ctype), _) => {
                if let Some(label_diff) = self.static_label_diff_initializer(init, *ctype) {
                    builder.put(base_offset, label_diff)?;
                    return Ok(());
                }
                if let Some(pointer_diff) = self.static_pointer_diff_integer(init) {
                    builder.put(base_offset, make_static_init(pointer_diff, *ctype))?;
                    return Ok(());
                }
                if matches!(ctype, CType::Int128 | CType::UInt128) {
                    let value = eval_static_wide_integer_constant_exp_with_context_and_values(
                        init,
                        &self.struct_defs,
                        &self.full_types,
                        &self.static_const_values,
                        &self.static_wide_const_values,
                    )
                    .ok_or_else(|| "Static variable initializer must be a constant".to_string())?;
                    builder.put(
                        base_offset,
                        make_static_wide_integer_init(
                            cast_static_wide_integer(value, *ctype),
                            *ctype,
                        ),
                    )?;
                    return Ok(());
                }
                let v = self.eval_static_scalar_init_for_type(&Some(init.clone()), *ctype)?;
                let cv = convert_init_value(v, *ctype);
                builder.put(base_offset, make_static_init(cv, *ctype))?;
            }
            _ => {
                return Err(format!(
                    "unsupported static aggregate initializer for {:?}",
                    base_ft
                ))
            }
        }
        Ok(())
    }

    pub(super) fn build_static_initializer(
        &mut self,
        ft: &FullType,
        init: &Exp,
    ) -> TackyResult<Vec<StaticInit>> {
        let mut builder = StaticInitBuilder::new();
        self.put_static_initializer(&mut builder, ft, init, 0)?;
        let total_bytes = ft
            .byte_size_with(&self.struct_defs)
            .max(builder.required_bytes());
        builder.finish(total_bytes)
    }

    pub(super) fn eval_static_constant_init(
        &self,
        init: &Option<Exp>,
    ) -> TackyResult<StaticScalarValue> {
        if let Some(exp) = init {
            eval_static_integer_constant_exp_with_context_and_values(
                exp,
                &self.struct_defs,
                &self.full_types,
                &self.static_const_values,
            )
            .map(StaticIntegerConstant::as_scalar_value)
            .ok_or_else(|| "Static variable initializer must be a constant".to_string())
        } else {
            Ok(StaticScalarValue::integer(0))
        }
    }

    pub(super) fn eval_static_scalar_init_for_type(
        &self,
        init: &Option<Exp>,
        target: CType,
    ) -> TackyResult<StaticScalarValue> {
        if target.is_floating() {
            return self.eval_static_constant_init(init);
        }
        let Some(exp) = init else {
            return Ok(StaticScalarValue::integer(0));
        };
        if let Some(value) = eval_static_integer_constant_exp_with_context_and_values(
            exp,
            &self.struct_defs,
            &self.full_types,
            &self.static_const_values,
        ) {
            return Ok(value.as_scalar_value());
        }
        if let Some(value) = eval_static_wide_integer_constant_exp_with_context_and_values(
            exp,
            &self.struct_defs,
            &self.full_types,
            &self.static_const_values,
            &self.static_wide_const_values,
        ) {
            return Ok(static_wide_integer_as_narrow_constant(value, target).as_scalar_value());
        }
        Err("Static variable initializer must be a constant".to_string())
    }

    pub(super) fn eval_static_complex_constant_init(
        &self,
        init: &Exp,
    ) -> Option<StaticComplexValue> {
        let zero = StaticScalarValue::integer(0);
        match init {
            Exp::ImaginaryIntConstant(value) => Some(StaticComplexValue {
                real: zero,
                imag: StaticScalarValue::integer(*value),
            }),
            Exp::ImaginaryDoubleConstant(value) => Some(StaticComplexValue {
                real: zero,
                imag: StaticScalarValue::double_bits(*value),
            }),
            Exp::Unary(UnaryOp::Negate, inner) => {
                let value = self.eval_static_complex_constant_init(inner)?;
                Some(StaticComplexValue {
                    real: neg_static_init_value(value.real),
                    imag: neg_static_init_value(value.imag),
                })
            }
            Exp::Binary(
                op @ (BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div),
                left,
                right,
            ) => {
                let left_value = self.eval_static_complex_constant_init(left)?;
                let right_value = self.eval_static_complex_constant_init(right)?;
                let left_real = static_init_value_to_f64(left_value.real);
                let left_imag = static_init_value_to_f64(left_value.imag);
                let right_real = static_init_value_to_f64(right_value.real);
                let right_imag = static_init_value_to_f64(right_value.imag);
                let (real, imag) = match op {
                    BinaryOp::Add => (left_real + right_real, left_imag + right_imag),
                    BinaryOp::Sub => (left_real - right_real, left_imag - right_imag),
                    BinaryOp::Mul => (
                        left_real * right_real - left_imag * right_imag,
                        left_real * right_imag + left_imag * right_real,
                    ),
                    BinaryOp::Div => {
                        let denom = right_real * right_real + right_imag * right_imag;
                        (
                            (left_real * right_real + left_imag * right_imag) / denom,
                            (left_imag * right_real - left_real * right_imag) / denom,
                        )
                    }
                    _ => unreachable!(),
                };
                Some(StaticComplexValue {
                    real: StaticScalarValue::double_bits(real),
                    imag: StaticScalarValue::double_bits(imag),
                })
            }
            _ => {
                let value = self.eval_static_constant_init(&Some(init.clone())).ok()?;
                Some(StaticComplexValue {
                    real: value,
                    imag: zero,
                })
            }
        }
    }

    pub(super) fn put_static_initializer_list(
        &mut self,
        builder: &mut StaticInitBuilder,
        base_ft: &FullType,
        elems: &[Exp],
        index: &mut usize,
        base_offset: usize,
    ) -> TackyResult<()> {
        if *index >= elems.len() {
            return Ok(());
        }

        match base_ft {
            FullType::Array { elem, size } => {
                self.reject_flexible_array_struct_array_initializer(base_ft)?;
                if *index < elems.len()
                    && matches!(elems[*index], Exp::StringLiteral(_))
                    && Self::is_one_dimensional_char_array(base_ft)
                {
                    self.put_static_initializer(builder, base_ft, &elems[*index], base_offset)?;
                    *index += 1;
                    return Ok(());
                }
                if *index < elems.len() && matches!(elems[*index], Exp::DesignatedInit(_, _)) {
                    self.put_static_initializer(builder, base_ft, &elems[*index], base_offset)?;
                    *index += 1;
                    return Ok(());
                }
                let elem_size = elem.byte_size_with(&self.struct_defs);
                if *size == 0 {
                    if *index >= elems.len() {
                        return Ok(());
                    }
                    if let Exp::ArrayInit(inner_elems) = &elems[*index] {
                        let mut inner_index = 0usize;
                        let mut elem_index = 0usize;
                        while inner_index < inner_elems.len() {
                            self.put_static_initializer_list(
                                builder,
                                elem,
                                inner_elems,
                                &mut inner_index,
                                base_offset + elem_index * elem_size,
                            )?;
                            elem_index += 1;
                        }
                        *index += 1;
                    } else {
                        let mut elem_index = 0usize;
                        while *index < elems.len() {
                            self.put_static_initializer_list(
                                builder,
                                elem,
                                elems,
                                index,
                                base_offset + elem_index * elem_size,
                            )?;
                            elem_index += 1;
                        }
                    }
                    return Ok(());
                }
                for i in 0..*size {
                    if *index >= elems.len() {
                        break;
                    }
                    let elem_init = &elems[*index];
                    if (Self::static_aggregate_initializer(elem_init).is_some()
                        || matches!(elem_init, Exp::StringLiteral(_)))
                        && (elem.is_array() || elem.is_struct())
                    {
                        self.put_static_initializer(
                            builder,
                            elem,
                            elem_init,
                            base_offset + i * elem_size,
                        )?;
                        *index += 1;
                    } else {
                        self.put_static_initializer_list(
                            builder,
                            elem,
                            elems,
                            index,
                            base_offset + i * elem_size,
                        )?;
                    }
                }
            }
            FullType::Struct(tag) => {
                let def = self
                    .struct_defs
                    .get(tag)
                    .cloned()
                    .ok_or_else(|| format!("Undefined struct: {}", tag))?;
                let max_members = if def.is_union { 1 } else { def.members.len() };
                for mem in def.members.iter().take(max_members) {
                    if *index >= elems.len() {
                        break;
                    }
                    let elem_init = &elems[*index];
                    if let Some(width) = mem.bit_width {
                        let value = self.eval_static_scalar_init_for_type(
                            &Some(elem_init.clone()),
                            mem.member_type,
                        )?;
                        builder.put_bit_field(
                            base_offset + mem.offset,
                            mem.member_type,
                            value.value,
                            mem.bit_offset,
                            width,
                        )?;
                        *index += 1;
                        continue;
                    }
                    if (Self::static_aggregate_initializer(elem_init).is_some()
                        || matches!(elem_init, Exp::StringLiteral(_)))
                        && (mem.member_full_type.is_array() || mem.member_full_type.is_struct())
                    {
                        self.put_static_initializer(
                            builder,
                            &mem.member_full_type,
                            elem_init,
                            base_offset + mem.offset,
                        )?;
                        *index += 1;
                    } else {
                        self.put_static_initializer_list(
                            builder,
                            &mem.member_full_type,
                            elems,
                            index,
                            base_offset + mem.offset,
                        )?;
                    }
                }
            }
            FullType::Pointer(_) | FullType::Scalar(_) | FullType::Vector { .. } => {
                self.put_static_initializer(builder, base_ft, &elems[*index], base_offset)?;
                *index += 1;
            }
            FullType::Function { .. } => {
                return Err("function type is not a static initializer target".to_string());
            }
        }
        Ok(())
    }

    /// Handle a variable declaration (arrays, scalars, static, etc.)
    pub(super) fn emit_var_decl(&mut self, vd: VarDeclaration) -> TackyResult<()> {
        if let Some(alignment) = vd.alignment {
            self.symbol_alignments
                .insert(vd.name.clone(), alignment.get());
        }
        let is_thread_local = vd
            .storage_class
            .as_ref()
            .is_some_and(StorageClass::is_thread_local);
        // Static arrays
        if vd.array_dims.is_some()
            && vd
                .storage_class
                .as_ref()
                .is_some_and(StorageClass::is_static)
        {
            let base_type = vd.var_type;
            let full_type = vd
                .decl_full_type
                .clone()
                .unwrap_or_else(|| FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims));
            let total_bytes = full_type.byte_size_with(&self.struct_defs);
            let align = {
                let elem_align = {
                    let mut t = &full_type;
                    while let FullType::Array { elem, .. } = t {
                        t = elem;
                    }
                    if let FullType::Struct(tag) = t {
                        self.struct_defs.get(tag).map(|d| d.alignment).unwrap_or(1)
                    } else {
                        std::cmp::max(base_type.size() as usize, 1)
                    }
                };
                if total_bytes >= 16 {
                    std::cmp::max(elem_align, 16)
                } else {
                    std::cmp::max(elem_align, 1)
                }
            };
            let align = vd.alignment.map_or(align, |a| a.get().max(align));
            self.register_var(&vd.name, full_type.clone());
            self.register_dynamic_size(&vd.name, vd.dynamic_size.clone());
            let init_values = if let Some(ref init_exp) = vd.init {
                self.build_static_initializer(&full_type, init_exp)?
            } else {
                vec![StaticInit::ZeroInit(total_bytes)]
            };
            self.static_vars.push(TackyStaticVar {
                name: vd.name.clone(),
                global: false,
                thread_local: is_thread_local,
                alignment: align,
                init_values,
            });
            return Ok(());
        }

        // Local arrays
        if vd.array_dims.is_some() {
            let base_type = vd.var_type;
            let full_type = vd
                .decl_full_type
                .clone()
                .unwrap_or_else(|| FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims));
            let total_bytes = full_type.byte_size_with(&self.struct_defs);
            self.register_var(&vd.name, full_type.clone());
            self.register_dynamic_size(&vd.name, vd.dynamic_size.clone());
            self.array_sizes.insert(vd.name.clone(), total_bytes);
            let scalar_type = {
                let mut t = &full_type;
                while let FullType::Array { elem, .. } = t {
                    t = elem;
                }
                t.to_ctype()
            };
            // Aggregate initializers zero-fill omitted elements; uninitialized automatic arrays do not.
            if vd.init.is_some() {
                self.zero_init_local(&vd.name, total_bytes);
            }
            if let Some(s) = vd
                .init
                .as_ref()
                .and_then(Self::string_array_initializer)
                .filter(|_| Self::is_one_dimensional_char_array(&full_type))
            {
                // String literal initializer for local char array: emit byte by byte
                let bytes = c_string_bytes(s);
                let chars_to_copy = std::cmp::min(bytes.len(), total_bytes);
                for (i, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                    let char_type = if base_type == CType::UChar {
                        CType::UChar
                    } else {
                        CType::Char
                    };
                    let src = self.fresh_tmp(char_type);
                    self.emit(TackyInstr::Copy {
                        src: TackyVal::Constant(byte as i64),
                        dst: src.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src,
                        dst_name: vd.name.clone(),
                        offset: i as i64,
                    });
                }
                // Null terminator if there's room (already zero-filled above)
            } else if let Some(init) = vd.init {
                if let Some(elem) = Self::direct_array_struct_elem(&full_type) {
                    let tag = elem.tag.to_string();
                    self.emit_struct_array_init_flat(&vd.name, &init, &tag, elem.array_len, 0)?;
                    return Ok(());
                }
                if let Exp::ArrayInit(elems) = &init {
                    let mut index = 0usize;
                    self.emit_initializer_list_at(&vd.name, &full_type, elems, &mut index, 0)?;
                    return Ok(());
                }
                let elem_sizes = Self::compute_elem_sizes(&full_type, &self.struct_defs);
                self.emit_array_init_flat(&vd.name, &init, scalar_type, 0, &elem_sizes)?;
            }
            return Ok(());
        }

        // Struct variable
        if let Some(FullType::Struct(ref tag)) = vd.decl_full_type {
            let tag = tag.clone();
            let def = self
                .struct_defs
                .get(&tag)
                .cloned()
                .ok_or_else(|| format!("Undefined struct: {}", tag))?;
            let struct_size = def.size;
            let struct_align = def.alignment;
            let ft = FullType::Struct(tag.clone());

            if vd
                .storage_class
                .as_ref()
                .is_some_and(StorageClass::is_static)
            {
                self.register_var(&vd.name, ft.clone());
                self.register_dynamic_size(&vd.name, vd.dynamic_size.clone());
                self.array_sizes.insert(vd.name.clone(), struct_size);
                let init_values = if let Some(ref init) = vd.init {
                    self.build_static_initializer(&ft, init)?
                } else {
                    vec![StaticInit::ZeroInit(struct_size)]
                };
                self.static_vars.push(TackyStaticVar {
                    name: vd.name.clone(),
                    global: false,
                    thread_local: is_thread_local,
                    alignment: vd
                        .alignment
                        .map_or(struct_align, |a| a.get().max(struct_align)),
                    init_values,
                });
                return Ok(());
            }

            if vd
                .storage_class
                .as_ref()
                .is_some_and(StorageClass::is_extern)
            {
                self.register_var(&vd.name, ft);
                self.register_dynamic_size(&vd.name, vd.dynamic_size.clone());
                self.extern_vars.push(vd.name);
                return Ok(());
            }
            self.register_var(&vd.name, ft);
            self.register_dynamic_size(&vd.name, vd.dynamic_size.clone());
            self.array_sizes.insert(vd.name.clone(), struct_size);
            // Aggregate initializers zero-fill omitted members; uninitialized automatic structs do not.
            if vd.init.is_some() {
                self.zero_init_local(&vd.name, struct_size);
            }
            // Handle compound initializer
            if let Some(init_ref @ Exp::ArrayInit(elems)) = vd.init.as_ref() {
                if elems
                    .iter()
                    .any(|elem| matches!(elem, Exp::DesignatedInit(_, _)))
                {
                    self.emit_struct_init_at(&vd.name, init_ref, &tag, 0)?;
                    return Ok(());
                }
                // For unions, if first member is array/struct, delegate the whole init
                if def.is_union && !def.members.is_empty() {
                    if elems.is_empty() {
                        return Ok(());
                    }
                    // For unions, the whole compound init {x, y, ...} initializes
                    // the FIRST MEMBER only. All elements go to the first member.
                    let mem = &def.members[0];
                    if mem.member_full_type.is_array() {
                        // Check if the first (and only) element is a string literal
                        if elems.len() == 1 {
                            if let Exp::StringLiteral(ref s) = elems[0] {
                                let bytes = c_string_bytes(s);
                                let chars_to_copy = std::cmp::min(bytes.len(), mem.size);
                                for (j, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                                    let src = self.fresh_tmp(CType::Char);
                                    self.emit(TackyInstr::Copy {
                                        src: TackyVal::Constant(byte as i64),
                                        dst: src.clone(),
                                    });
                                    self.emit(TackyInstr::CopyToOffset {
                                        src,
                                        dst_name: vd.name.clone(),
                                        offset: j as i64,
                                    });
                                }
                            } else {
                                // Single ArrayInit element → pass to array init for the array member
                                let mem_elem_sizes = Self::compute_elem_sizes(
                                    &mem.member_full_type,
                                    &self.struct_defs,
                                );
                                let inner_scalar = {
                                    let mut t = &mem.member_full_type;
                                    while let FullType::Array { elem: e, .. } = t {
                                        t = e;
                                    }
                                    t.to_ctype()
                                };
                                self.emit_array_init_flat(
                                    &vd.name,
                                    &elems[0],
                                    inner_scalar,
                                    0,
                                    &mem_elem_sizes,
                                )?;
                            }
                        } else {
                            // Multiple elements → they're all array element inits
                            let mem_elem_sizes =
                                Self::compute_elem_sizes(&mem.member_full_type, &self.struct_defs);
                            let inner_scalar = {
                                let mut t = &mem.member_full_type;
                                while let FullType::Array { elem: e, .. } = t {
                                    t = e;
                                }
                                t.to_ctype()
                            };
                            self.emit_array_init_flat(
                                &vd.name,
                                init_ref,
                                inner_scalar,
                                0,
                                &mem_elem_sizes,
                            )?;
                        }
                    } else if mem.member_full_type.is_struct() {
                        if let FullType::Struct(ref inner_tag) = mem.member_full_type {
                            let first = &elems[0];
                            if let Exp::ArrayInit(_) = first {
                                // Compound init for the struct member
                                if elems.len() == 1 {
                                    self.emit_struct_init_at(&vd.name, first, inner_tag, 0)?;
                                } else {
                                    self.emit_struct_init_at(&vd.name, init_ref, inner_tag, 0)?;
                                }
                            } else {
                                // Struct-valued expression (variable, etc.) — struct copy
                                let struct_size =
                                    mem.member_full_type.byte_size_with(&self.struct_defs);
                                let (val, val_type) = self.emit_exp(first.clone())?;
                                let src_addr = if val_type == CType::Pointer {
                                    val
                                } else {
                                    self.get_struct_addr(val)
                                };
                                self.emit_struct_copy_to(src_addr, &vd.name, struct_size);
                            }
                        }
                    } else {
                        // Scalar first member — use first element
                        self.assert_pointer_initializer_assignable(
                            &mem.member_full_type,
                            &elems[0],
                        )?;
                        let (val, val_type) = self.emit_exp(elems[0].clone())?;
                        let val_conv = self.convert_to(val, val_type, mem.member_type);
                        if mem.bit_width.is_some() {
                            self.store_bit_field_to_offset(vd.name.clone(), mem, val_conv)?;
                        } else {
                            self.emit(TackyInstr::CopyToOffset {
                                src: val_conv,
                                dst_name: vd.name.clone(),
                                offset: 0,
                            });
                        }
                    }
                } else {
                    let max_members = def.members.len();
                    if max_members == 1 && def.members[0].member_full_type.is_array() {
                        let member = &def.members[0];
                        let elem_sizes =
                            Self::compute_elem_sizes(&member.member_full_type, &self.struct_defs);
                        let scalar_t = {
                            let mut t = &member.member_full_type;
                            while let FullType::Array { elem: e, .. } = t {
                                t = e;
                            }
                            t.to_ctype()
                        };
                        if let [single] = elems.as_slice() {
                            if matches!(single, Exp::StringLiteral(_)) {
                                self.emit_array_init_flat(
                                    &vd.name,
                                    single,
                                    scalar_t,
                                    member.offset as i64,
                                    &elem_sizes,
                                )?;
                                return Ok(());
                            }
                        }
                        self.emit_array_init_flat(
                            &vd.name,
                            init_ref,
                            scalar_t,
                            member.offset as i64,
                            &elem_sizes,
                        )?;
                        return Ok(());
                    }
                    for (i, elem) in elems.iter().enumerate() {
                        if i >= max_members {
                            break;
                        }
                        let member = &def.members[i];
                        let mem_ft = &member.member_full_type;
                        // Handle nested struct/array member init
                        if mem_ft.is_array() || mem_ft.is_struct() {
                            // Handle string literal for char array members
                            if let Exp::StringLiteral(ref s) = elem {
                                let bytes = c_string_bytes(s);
                                let chars_to_copy = std::cmp::min(bytes.len(), member.size);
                                for (j, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                                    let char_type = CType::Char;
                                    let src = self.fresh_tmp(char_type);
                                    self.emit(TackyInstr::Copy {
                                        src: TackyVal::Constant(byte as i64),
                                        dst: src.clone(),
                                    });
                                    self.emit(TackyInstr::CopyToOffset {
                                        src,
                                        dst_name: vd.name.clone(),
                                        offset: (member.offset + j) as i64,
                                    });
                                }
                            } else if mem_ft.is_struct()
                                && !matches!(elem, Exp::ArrayInit(_) | Exp::StringLiteral(_))
                            {
                                // Struct member initialized with a struct-valued expression (e.g., a variable)
                                let struct_size = mem_ft.byte_size_with(&self.struct_defs);
                                let (val, val_type) = self.emit_exp(elem.clone())?;
                                let src_addr = if val_type == CType::Pointer {
                                    val
                                } else {
                                    self.get_struct_addr(val)
                                };
                                // Copy struct data to the member offset
                                let dst_addr = self.fresh_tmp(CType::Pointer);
                                self.emit(TackyInstr::GetAddress {
                                    src: TackyVal::Var(vd.name.clone()),
                                    dst: dst_addr.clone(),
                                });
                                let member_addr = self.fresh_tmp(CType::Pointer);
                                if member.offset > 0 {
                                    self.emit(TackyInstr::Binary {
                                        op: TackyBinaryOp::Add,
                                        left: dst_addr,
                                        right: TackyVal::Constant(member.offset as i64),
                                        dst: member_addr.clone(),
                                    });
                                } else {
                                    self.emit(TackyInstr::Copy {
                                        src: dst_addr,
                                        dst: member_addr.clone(),
                                    });
                                }
                                self.emit_struct_copy_ptr_to_ptr(
                                    src_addr,
                                    member_addr,
                                    struct_size,
                                );
                            } else if let Exp::ArrayInit(ref sub_elems) = elem {
                                if mem_ft.is_array() {
                                    if let FullType::Array {
                                        elem: array_elem,
                                        size,
                                    } = mem_ft
                                    {
                                        if let FullType::Struct(array_tag) = array_elem.as_ref() {
                                            let array_tag = array_tag.clone();
                                            self.emit_struct_array_init_flat(
                                                &vd.name,
                                                elem,
                                                &array_tag,
                                                *size,
                                                member.offset as i64,
                                            )?;
                                        } else {
                                            let elem_sizes =
                                                Self::compute_elem_sizes(mem_ft, &self.struct_defs);
                                            let scalar_t = {
                                                let mut t = mem_ft;
                                                while let FullType::Array { elem: e, .. } = t {
                                                    t = e;
                                                }
                                                t.to_ctype()
                                            };
                                            self.emit_array_init_flat(
                                                &vd.name,
                                                elem,
                                                scalar_t,
                                                member.offset as i64,
                                                &elem_sizes,
                                            )?;
                                        }
                                    }
                                } else if let FullType::Struct(ref inner_tag) = mem_ft {
                                    // Nested struct compound init
                                    let inner_def =
                                        self.struct_defs.get(inner_tag).cloned().ok_or_else(
                                            || format!("Undefined struct: {}", inner_tag),
                                        )?;
                                    for (j, sub_elem) in sub_elems.iter().enumerate() {
                                        if j >= inner_def.members.len() {
                                            break;
                                        }
                                        let inner_mem = &inner_def.members[j];
                                        if inner_mem.member_full_type.is_array() {
                                            let elem_sizes = Self::compute_elem_sizes(
                                                &inner_mem.member_full_type,
                                                &self.struct_defs,
                                            );
                                            let scalar_t = {
                                                let mut t = &inner_mem.member_full_type;
                                                while let FullType::Array { elem: e, .. } = t {
                                                    t = e;
                                                }
                                                t.to_ctype()
                                            };
                                            self.emit_array_init_flat(
                                                &vd.name,
                                                sub_elem,
                                                scalar_t,
                                                (member.offset + inner_mem.offset) as i64,
                                                &elem_sizes,
                                            )?;
                                        } else if inner_mem.member_full_type.is_struct() {
                                            if let (
                                                Exp::ArrayInit(_),
                                                FullType::Struct(ref nested_tag),
                                            ) = (sub_elem, &inner_mem.member_full_type)
                                            {
                                                self.emit_struct_init_at(
                                                    &vd.name,
                                                    sub_elem,
                                                    nested_tag,
                                                    (member.offset + inner_mem.offset) as i64,
                                                )?;
                                            } else {
                                                self.assert_pointer_initializer_assignable(
                                                    &inner_mem.member_full_type,
                                                    sub_elem,
                                                )?;
                                                let (val, val_type) =
                                                    self.emit_exp(sub_elem.clone())?;
                                                let target_type = inner_mem.member_type;
                                                let val_conv =
                                                    self.convert_to(val, val_type, target_type);
                                                self.emit(TackyInstr::CopyToOffset {
                                                    src: val_conv,
                                                    dst_name: vd.name.clone(),
                                                    offset: (member.offset + inner_mem.offset)
                                                        as i64,
                                                });
                                            }
                                        } else {
                                            self.assert_pointer_initializer_assignable(
                                                &inner_mem.member_full_type,
                                                sub_elem,
                                            )?;
                                            let (val, val_type) =
                                                self.emit_exp(sub_elem.clone())?;
                                            let target_type = inner_mem.member_type;
                                            let val_conv =
                                                self.convert_to(val, val_type, target_type);
                                            self.emit(TackyInstr::CopyToOffset {
                                                src: val_conv,
                                                dst_name: vd.name.clone(),
                                                offset: (member.offset + inner_mem.offset) as i64,
                                            });
                                        }
                                    }
                                }
                            }
                        } else {
                            self.assert_pointer_initializer_assignable(
                                &member.member_full_type,
                                elem,
                            )?;
                            let (val, val_type) = self.emit_exp(elem.clone())?;
                            let target_type = member.member_type;
                            let val_conv = self.convert_to(val, val_type, target_type);
                            if member.bit_width.is_some() {
                                self.store_bit_field_to_offset(vd.name.clone(), member, val_conv)?;
                            } else {
                                self.emit(TackyInstr::CopyToOffset {
                                    src: val_conv,
                                    dst_name: vd.name.clone(),
                                    offset: member.offset as i64,
                                });
                            }
                        }
                    }
                } // end else (non-union compound init)
            } else if let Some(init) = vd.init {
                // Copy from another struct expression
                let (val, val_type) = self.emit_exp(init)?;
                let rhs_struct_name = if val_type == CType::Struct {
                    if let TackyVal::Var(ref n) = val {
                        Some(n.clone())
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(src_name) = rhs_struct_name {
                    self.emit(TackyInstr::CopyStruct {
                        src_name,
                        dst_name: vd.name.clone(),
                    });
                } else {
                    let src_addr = if val_type == CType::Pointer {
                        val
                    } else {
                        self.get_struct_addr(val)
                    };
                    self.emit_struct_copy_to(src_addr, &vd.name, struct_size);
                }
            }
            return Ok(());
        }

        // Use decl_full_type if available (preserves pointer-to-array info)
        let ft = if let Some(ref dft) = vd.decl_full_type {
            dft.clone()
        } else {
            FullType::from_decl(vd.var_type, vd.ptr_info, &None)
        };
        // Regular scalar/pointer variable. Vectors keep their full type for
        // lane operations, but use a storage-width scalar for backend moves.
        let storage_type = self.storage_ctype_for_full(&ft);
        self.var_types.insert(vd.name.clone(), storage_type);
        self.symbol_types.insert(vd.name.clone(), storage_type);
        if let Some(pi) = vd.ptr_info {
            self.ptr_info.insert(vd.name.clone(), pi);
        }
        self.full_types.insert(vd.name.clone(), ft.clone());
        self.register_dynamic_size(&vd.name, vd.dynamic_size.clone());
        if ft.is_vector() {
            self.array_sizes
                .insert(vd.name.clone(), ft.byte_size_with(&self.struct_defs));
        }
        let target_ft = ft.clone();
        let declaration_is_pointer =
            matches!(target_ft, FullType::Pointer(_)) || vd.var_type == CType::Pointer;
        let init = match vd.init {
            Some(Exp::ArrayInit(mut elems)) if elems.len() == 1 => Some(elems.remove(0)),
            Some(Exp::ArrayInit(elems)) if elems.is_empty() => Some(Exp::Constant(0)),
            other => other,
        };

        if vd
            .storage_class
            .as_ref()
            .is_some_and(StorageClass::is_static)
        {
            // Static pointer initialized with string literal: static char *p = "hello";
            if let (true, Some(Exp::StringLiteral(ref s))) = (declaration_is_pointer, &init) {
                self.assert_static_pointer_initializer_assignable(
                    &target_ft,
                    init.as_ref().expect("string initializer exists"),
                )?;
                let str_label = self.make_string_constant(s);
                let align = std::cmp::max(vd.var_type.size() as usize, 1);
                let align = vd.alignment.map_or(align, |a| a.get().max(align));
                self.static_vars.push(TackyStaticVar {
                    name: vd.name,
                    global: false,
                    thread_local: is_thread_local,
                    alignment: align,
                    init_values: vec![StaticInit::PointerInit(str_label)],
                });
                return Ok(());
            }
            if declaration_is_pointer {
                if let Some(init) = init.as_ref() {
                    self.assert_static_pointer_initializer_assignable(&target_ft, init)?;
                    if let Some(ptr_init) = self.static_pointer_initializer(init) {
                        let align = std::cmp::max(vd.var_type.size() as usize, 1);
                        let align = vd.alignment.map_or(align, |a| a.get().max(align));
                        self.static_vars.push(TackyStaticVar {
                            name: vd.name,
                            global: false,
                            thread_local: is_thread_local,
                            alignment: align,
                            init_values: vec![ptr_init],
                        });
                        return Ok(());
                    }
                }
            }
            let align = if vd.var_type == CType::Double {
                16
            } else {
                std::cmp::max(vd.var_type.size() as usize, 1)
            };
            let align = vd.alignment.map_or(align, |a| a.get().max(align));
            let init_v = if matches!(vd.var_type, CType::Int128 | CType::UInt128) {
                let value = if let Some(init) = init.as_ref() {
                    eval_static_wide_integer_constant_exp_with_context_and_values(
                        init,
                        &self.struct_defs,
                        &self.full_types,
                        &self.static_const_values,
                        &self.static_wide_const_values,
                    )
                    .ok_or_else(|| "Static variable initializer must be a constant".to_string())?
                } else {
                    StaticWideIntegerConstant::new(0, !vd.var_type.is_signed())
                };
                make_static_wide_integer_init(
                    cast_static_wide_integer(value, vd.var_type),
                    vd.var_type,
                )
            } else {
                let raw_val = self.eval_static_scalar_init_for_type(&init, vd.var_type)?;
                let init_val = convert_init_value(raw_val, vd.var_type);
                make_static_init(init_val, vd.var_type)
            };
            self.static_vars.push(TackyStaticVar {
                name: vd.name,
                global: false,
                thread_local: is_thread_local,
                alignment: align,
                init_values: vec![init_v],
            });
        } else if vd
            .storage_class
            .as_ref()
            .is_some_and(StorageClass::is_extern)
        {
            self.extern_vars.push(vd.name);
        } else if let Some(init) = init {
            let vd_name = vd.name.clone();
            let init_for_type = init.clone();
            if ft.is_complex() {
                let size = ft.byte_size_with(&self.struct_defs);
                self.zero_init_local(&vd_name, size);
                if let Exp::ArrayInit(elems) = init {
                    let (elem_ft, elem_type, elem_size) = match &ft {
                        FullType::Vector { elem, .. } => (
                            elem.as_ref().clone(),
                            elem.to_ctype(),
                            elem.byte_size_with(&self.struct_defs),
                        ),
                        _ => (
                            FullType::Scalar(vd.var_type),
                            vd.var_type,
                            vd.var_type.size() as usize,
                        ),
                    };
                    for (index, elem_init) in elems.into_iter().take(2).enumerate() {
                        let (val, val_type) = self.emit_exp(elem_init.clone())?;
                        let val_ft = self.val_full_type(&val);
                        self.assert_assignable_exp_full_type(
                            &elem_ft,
                            &val_ft,
                            &elem_init,
                            "initializer",
                        )?;
                        let converted = self.convert_to(val, val_type, elem_type);
                        self.emit(TackyInstr::CopyToOffset {
                            src: converted,
                            dst_name: vd_name.clone(),
                            offset: (index * elem_size) as i64,
                        });
                    }
                    return Ok(());
                }
                let (val, val_type) = self.emit_exp(init)?;
                let val_ft = self.val_full_type(&val);
                self.assert_assignable_exp_full_type(&ft, &val_ft, &init_for_type, "initializer")?;
                if val_ft.is_complex() {
                    let src_addr = self.get_struct_addr(val);
                    self.emit_struct_copy_to(src_addr, &vd_name, size);
                } else {
                    let elem_type = match &ft {
                        FullType::Vector { elem, .. } => elem.to_ctype(),
                        _ => vd.var_type,
                    };
                    let real = self.convert_to(val, val_type, elem_type);
                    self.emit(TackyInstr::CopyToOffset {
                        src: real,
                        dst_name: vd_name.clone(),
                        offset: 0,
                    });
                }
                return Ok(());
            }
            if ft.is_vector() {
                let size = ft.byte_size_with(&self.struct_defs);
                self.zero_init_local(&vd_name, size);
                if let Exp::ArrayInit(elems) = init {
                    let (elem_ft, elem_type, elem_size) = match &ft {
                        FullType::Vector { elem, .. } => (
                            elem.as_ref().clone(),
                            elem.to_ctype(),
                            elem.byte_size_with(&self.struct_defs),
                        ),
                        _ => (
                            FullType::Scalar(vd.var_type),
                            vd.var_type,
                            vd.var_type.size() as usize,
                        ),
                    };
                    for (index, elem_init) in elems.into_iter().enumerate() {
                        let (val, val_type) = self.emit_exp(elem_init.clone())?;
                        let val_ft = self.val_full_type(&val);
                        self.assert_assignable_exp_full_type(
                            &elem_ft,
                            &val_ft,
                            &elem_init,
                            "initializer",
                        )?;
                        let converted = self.convert_to(val, val_type, elem_type);
                        self.emit(TackyInstr::CopyToOffset {
                            src: converted,
                            dst_name: vd_name.clone(),
                            offset: (index * elem_size) as i64,
                        });
                    }
                    return Ok(());
                }
            }
            let (val, val_type) = self.emit_exp(init)?;
            let val_ft = self.val_full_type(&val);
            self.assert_assignable_exp_full_type(&ft, &val_ft, &init_for_type, "initializer")?;
            if ft.is_vector() {
                let size = ft.byte_size_with(&self.struct_defs);
                let src_addr = if let TackyVal::Var(ref src_name) = val {
                    if self.array_sizes.contains_key(src_name) {
                        let addr = self.fresh_tmp(CType::Pointer);
                        self.emit(TackyInstr::GetAddress {
                            src: val,
                            dst: addr.clone(),
                        });
                        addr
                    } else {
                        self.zero_init_local(&vd_name, size);
                        let converted = self.convert_to(val, val_type, ft.to_ctype());
                        self.emit(TackyInstr::CopyToOffset {
                            src: converted,
                            dst_name: vd_name.clone(),
                            offset: 0,
                        });
                        return Ok(());
                    }
                } else {
                    self.zero_init_local(&vd_name, size);
                    let converted = self.convert_to(val, val_type, ft.to_ctype());
                    self.emit(TackyInstr::CopyToOffset {
                        src: converted,
                        dst_name: vd_name.clone(),
                        offset: 0,
                    });
                    return Ok(());
                };
                self.emit_struct_copy_to(src_addr, &vd_name, size);
                return Ok(());
            }
            let val_conv = self.convert_to(val, val_type, vd.var_type);
            if vd.var_type == CType::Pointer {
                if let TackyVal::Var(ref src_name) = val_conv {
                    // Only propagate ptr_info if LHS doesn't have specific decl info
                    if vd.decl_full_type.is_none() {
                        if let Some(&info) = self.ptr_info.get(src_name) {
                            self.ptr_info.insert(vd_name.clone(), info);
                        }
                        if let Some(ft) = self.full_types.get(src_name).cloned() {
                            self.full_types.insert(vd_name.clone(), ft);
                        }
                    }
                }
            }
            self.emit(TackyInstr::Copy {
                src: val_conv,
                dst: TackyVal::Var(vd_name),
            });
        }
        Ok(())
    }

    pub(super) fn emit_block(&mut self, block: Block) -> TackyResult<()> {
        for item in block {
            match item {
                BlockItem::Declaration(Declaration::VarDecl(vd)) => {
                    self.emit_var_decl(vd)?;
                }
                BlockItem::Declaration(Declaration::FunDecl(fd)) => {
                    // Register function type for block-scope prototypes
                    self.function_symbols.insert(fd.name.clone());
                    let param_types: Vec<CType> = fd.params.iter().map(|(_, t, _)| *t).collect();
                    self.func_types.insert(
                        fd.name.clone(),
                        (fd.return_type, param_types, fd.return_ptr_info, fd.variadic),
                    );
                    if fd.old_style {
                        self.old_style_functions.insert(fd.name.clone());
                    } else {
                        self.old_style_functions.remove(&fd.name);
                    }
                    self.record_zero_fixed_variadic_function(&fd);
                    self.func_param_full_types
                        .insert(fd.name.clone(), fd.param_full_types.clone());
                    if let Some(ref rft) = fd.return_full_type {
                        self.func_full_types.insert(fd.name.clone(), rft.clone());
                    }
                    if fd.body.is_some() {
                        self.emit_nested_function(fd)?;
                    }
                }
                BlockItem::Declaration(Declaration::StructDecl(sd)) => {
                    if sd.members.is_empty() && self.struct_defs.contains_key(&sd.tag) {
                        continue;
                    }
                    if sd.is_union && sd.transparent_union {
                        if let Some(member) = sd.members.first() {
                            self.transparent_unions
                                .insert(sd.tag.clone(), member.member_full_type.clone());
                        }
                    }
                    let def = StructDef::from_declaration(&sd, &self.struct_defs)?;
                    self.struct_defs.insert(sd.tag.clone(), def);
                }
                BlockItem::Declaration(Declaration::TypedefDecl) => {}
                BlockItem::Statement(stmt) => self.emit_statement(stmt)?,
            }
        }
        Ok(())
    }

    pub(super) fn emit_nested_capture_updates(&mut self, function_name: &str) {
        let Some(captures) = self.nested_capture_slots.get(function_name).cloned() else {
            return;
        };
        for capture_slot in captures {
            self.ensure_current_capture_slot(&capture_slot.capture);
            let src = self
                .nested_capture_slots
                .get(&self.current_function)
                .and_then(|current_captures| {
                    current_captures.iter().find_map(|current_capture_slot| {
                        (current_capture_slot.capture == capture_slot.capture)
                            .then(|| TackyVal::Var(current_capture_slot.slot.clone()))
                    })
                });
            let src = if let Some(src) = src {
                src
            } else {
                let addr = self.fresh_tmp(CType::Pointer);
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(capture_slot.capture),
                    dst: addr.clone(),
                });
                addr
            };
            self.emit(TackyInstr::Copy {
                src,
                dst: TackyVal::Var(capture_slot.slot),
            });
        }
    }

    pub(super) fn ensure_current_capture_slot(&mut self, capture: &str) {
        if self.current_function.is_empty() || self.current_function_locals.contains(capture) {
            return;
        }
        if self.file_scope_symbols.contains(capture) {
            return;
        }
        if self
            .nested_capture_slots
            .get(&self.current_function)
            .is_some_and(|captures| {
                captures
                    .iter()
                    .any(|capture_slot| capture_slot.capture == capture)
            })
        {
            return;
        }
        let Some(captured_ft) = self.full_types.get(capture).cloned() else {
            return;
        };
        let slot = format!(
            "__rnqcc_chain_{}_{}",
            self.current_function,
            capture.replace('.', "_")
        );
        let slot_ft = FullType::Pointer(Box::new(captured_ft));
        self.register_var(&slot, slot_ft);
        self.static_vars.push(TackyStaticVar {
            name: slot.clone(),
            global: false,
            thread_local: false,
            alignment: 8,
            init_values: vec![StaticInit::ZeroInit(8)],
        });
        self.nested_capture_slots
            .entry(self.current_function.clone())
            .or_default()
            .push(NestedCaptureSlot {
                capture: capture.to_string(),
                slot,
            });
    }

    pub(super) fn emit_nested_function(&mut self, mut fd: FunctionDeclaration) -> TackyResult<()> {
        let mut nested_labels = HashSet::new();
        if let Some(body) = fd.body.as_ref() {
            Self::collect_block_labels(body, &mut nested_labels);
        }
        let mut parent_label_envs = HashMap::new();
        if let Some(body) = fd.body.as_ref() {
            let parent_labels: HashSet<String> =
                self.current_nonlocal_label_envs.keys().cloned().collect();
            let parent_gotos = Self::collect_parent_label_gotos_stmt(
                &Statement::Block(body.clone()),
                &nested_labels,
                &parent_labels,
            );
            for label in parent_gotos {
                if let Some(env) = self.current_nonlocal_label_envs.get(&label) {
                    parent_label_envs.insert(label, env.clone());
                }
            }
        }
        if self.current_escaped_functions.contains(&fd.name) {
            if let Some(body) = fd.body.take() {
                fd.body = Some(Self::rewrite_parent_label_gotos_block(
                    body,
                    &nested_labels,
                    &self.current_label_bodies,
                ));
            }
        }
        let mut captures = self.collect_captures_for_nested(&fd);
        for env in parent_label_envs.values() {
            if !captures.iter().any(|capture| capture == env) {
                captures.push(env.clone());
            }
        }
        let mut capture_map = HashMap::new();
        let mut capture_slots = Vec::new();
        let mut parent_label_env_slots = HashMap::new();
        for capture in captures {
            let Some(captured_ft) = self.full_types.get(&capture).cloned() else {
                continue;
            };
            let slot = format!("__rnqcc_chain_{}_{}", fd.name, capture.replace('.', "_"));
            let slot_ft = FullType::Pointer(Box::new(captured_ft));
            self.register_var(&slot, slot_ft);
            self.static_vars.push(TackyStaticVar {
                name: slot.clone(),
                global: false,
                thread_local: false,
                alignment: 8,
                init_values: vec![StaticInit::ZeroInit(8)],
            });
            capture_slots.push(NestedCaptureSlot {
                capture: capture.clone(),
                slot: slot.clone(),
            });
            capture_map.insert(capture, slot);
        }
        for (label, env) in parent_label_envs {
            if let Some(slot) = capture_map.get(&env) {
                parent_label_env_slots.insert(label, slot.clone());
            }
        }
        self.nested_capture_slots
            .insert(fd.name.clone(), capture_slots);
        if let Some(body) = fd.body.take() {
            fd.body = Some(Self::rewrite_capture_block(body, &capture_map));
        }

        let saved_instructions = std::mem::take(&mut self.instructions);
        let saved_current = std::mem::take(&mut self.current_function);
        let saved_current_params = std::mem::take(&mut self.current_function_params);
        let saved_current_locals = std::mem::take(&mut self.current_function_locals);
        let saved_label_function = self.label_address_function.take();
        let saved_label_bodies = std::mem::take(&mut self.current_label_bodies);
        let saved_escaped_functions = std::mem::take(&mut self.current_escaped_functions);
        let saved_nonlocal_label_envs = std::mem::take(&mut self.current_nonlocal_label_envs);
        let saved_parent_label_env_slots = std::mem::replace(
            &mut self.current_parent_label_env_slots,
            parent_label_env_slots,
        );
        let saved_hidden_ret = self.hidden_ret_ptr.take();
        if !saved_current.is_empty() {
            self.label_address_function = Some(saved_current.clone());
        }
        if let Some(mut nested) = self.emit_function(fd)? {
            nested.global = false;
            self.nested_functions.push(nested);
        }
        self.instructions = saved_instructions;
        self.current_function = saved_current;
        self.current_function_params = saved_current_params;
        self.current_function_locals = saved_current_locals;
        self.label_address_function = saved_label_function;
        self.current_label_bodies = saved_label_bodies;
        self.current_escaped_functions = saved_escaped_functions;
        self.current_nonlocal_label_envs = saved_nonlocal_label_envs;
        self.current_parent_label_env_slots = saved_parent_label_env_slots;
        self.hidden_ret_ptr = saved_hidden_ret;
        Ok(())
    }

    pub(super) fn rewrite_parent_label_gotos_block(
        block: Block,
        local_labels: &HashSet<String>,
        parent_labels: &IndexMap<String, Statement>,
    ) -> Block {
        block
            .into_iter()
            .map(|item| match item {
                BlockItem::Statement(stmt) => BlockItem::Statement(
                    Self::rewrite_parent_label_gotos_stmt(stmt, local_labels, parent_labels),
                ),
                other => other,
            })
            .collect()
    }

    pub(super) fn rewrite_parent_label_gotos_stmt(
        stmt: Statement,
        local_labels: &HashSet<String>,
        parent_labels: &IndexMap<String, Statement>,
    ) -> Statement {
        match stmt {
            Statement::Goto(label) if !local_labels.contains(&label) => parent_labels
                .get(&label)
                .cloned()
                .unwrap_or(Statement::Goto(label)),
            Statement::If(cond, then_stmt, else_stmt) => Statement::If(
                cond,
                Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *then_stmt,
                    local_labels,
                    parent_labels,
                )),
                else_stmt.map(|stmt| {
                    Box::new(Self::rewrite_parent_label_gotos_stmt(
                        *stmt,
                        local_labels,
                        parent_labels,
                    ))
                }),
            ),
            Statement::Block(block) => Statement::Block(Self::rewrite_parent_label_gotos_block(
                block,
                local_labels,
                parent_labels,
            )),
            Statement::While {
                condition,
                body,
                label,
            } => Statement::While {
                condition,
                body: Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
                label,
            },
            Statement::DoWhile {
                body,
                condition,
                label,
            } => Statement::DoWhile {
                body: Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
                condition,
                label,
            },
            Statement::For {
                init,
                condition,
                post,
                body,
                label,
            } => Statement::For {
                init,
                condition,
                post,
                body: Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
                label,
            },
            Statement::Label(label, body) => Statement::Label(
                label,
                Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
            ),
            Statement::Switch {
                control,
                body,
                label,
                cases,
            } => Statement::Switch {
                control,
                body: Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
                label,
                cases,
            },
            Statement::Case {
                value,
                end_value,
                body,
                label,
            } => Statement::Case {
                value,
                end_value,
                body: Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
                label,
            },
            Statement::Default { body, label } => Statement::Default {
                body: Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
                label,
            },
            other => other,
        }
    }

    pub(super) fn collect_captures_for_nested(&self, fd: &FunctionDeclaration) -> Vec<String> {
        let mut local_names: std::collections::HashSet<String> =
            fd.params.iter().map(|(name, _, _)| name.clone()).collect();
        if let Some(body) = fd.body.as_ref() {
            Self::collect_declared_names(body, &mut local_names);
        }
        let mut used = Vec::new();
        if let Some(body) = fd.body.as_ref() {
            Self::collect_used_vars_block(body, &mut used);
        }
        let mut captures: Vec<String> = used
            .iter()
            .filter(|name| {
                !local_names.contains(*name)
                    && self.full_types.contains_key(*name)
                    && !self.function_symbols.contains(*name)
            })
            .cloned()
            .collect();
        for name in used {
            if let Some(nested_captures) = self.nested_capture_slots.get(&name) {
                for capture_slot in nested_captures {
                    if !local_names.contains(&capture_slot.capture)
                        && self.full_types.contains_key(&capture_slot.capture)
                        && !captures
                            .iter()
                            .any(|existing| existing == &capture_slot.capture)
                    {
                        captures.push(capture_slot.capture.clone());
                    }
                }
            }
        }
        captures
    }

    pub(super) fn collect_declared_names(
        block: &Block,
        names: &mut std::collections::HashSet<String>,
    ) {
        for item in block {
            match item {
                BlockItem::Declaration(Declaration::VarDecl(vd)) => {
                    names.insert(vd.name.clone());
                }
                BlockItem::Declaration(Declaration::FunDecl(fd)) => {
                    names.insert(fd.name.clone());
                }
                BlockItem::Statement(stmt) => Self::collect_declared_names_stmt(stmt, names),
                _ => {}
            }
        }
    }

    pub(super) fn collect_declared_names_stmt(
        stmt: &Statement,
        names: &mut std::collections::HashSet<String>,
    ) {
        match stmt {
            Statement::Block(block) => Self::collect_declared_names(block, names),
            Statement::If(_, then_stmt, else_stmt) => {
                Self::collect_declared_names_stmt(then_stmt, names);
                if let Some(else_stmt) = else_stmt {
                    Self::collect_declared_names_stmt(else_stmt, names);
                }
            }
            Statement::While { body, .. }
            | Statement::DoWhile { body, .. }
            | Statement::Label(_, body)
            | Statement::Case { body, .. }
            | Statement::Default { body, .. }
            | Statement::Switch { body, .. } => Self::collect_declared_names_stmt(body, names),
            Statement::For { init, body, .. } => {
                if let ForInit::Declaration(vd) = init.as_ref() {
                    names.insert(vd.name.clone());
                }
                Self::collect_declared_names_stmt(body, names);
            }
            _ => {}
        }
    }

    pub(super) fn collect_used_vars_block(block: &Block, used: &mut Vec<String>) {
        for item in block {
            match item {
                BlockItem::Declaration(Declaration::VarDecl(vd)) => {
                    if let Some(init) = vd.init.as_ref() {
                        Self::collect_used_vars_exp(init, used);
                    }
                }
                BlockItem::Declaration(Declaration::FunDecl(_)) => {}
                BlockItem::Statement(stmt) => Self::collect_used_vars_stmt(stmt, used),
                _ => {}
            }
        }
    }

    pub(super) fn push_used_var(name: &str, used: &mut Vec<String>) {
        if !used.iter().any(|existing| existing == name) {
            used.push(name.to_string());
        }
    }

    pub(super) fn collect_used_vars_exp(exp: &Exp, used: &mut Vec<String>) {
        match exp {
            Exp::Var(name) => Self::push_used_var(name, used),
            Exp::Cast(_, _, inner)
            | Exp::Unary(_, inner)
            | Exp::SizeOf(inner)
            | Exp::Dot(inner, _)
            | Exp::Arrow(inner, _) => Self::collect_used_vars_exp(inner, used),
            Exp::Binary(_, left, right)
            | Exp::Assign(left, right)
            | Exp::CompoundAssign(_, left, right)
            | Exp::Subscript(left, right)
            | Exp::Comma(left, right) => {
                Self::collect_used_vars_exp(left, used);
                Self::collect_used_vars_exp(right, used);
            }
            Exp::Conditional(a, b, c) => {
                Self::collect_used_vars_exp(a, used);
                Self::collect_used_vars_exp(b, used);
                Self::collect_used_vars_exp(c, used);
            }
            Exp::BuiltinExpect(value, hints) => {
                Self::collect_used_vars_exp(value, used);
                for hint in hints {
                    Self::collect_used_vars_exp(hint, used);
                }
            }
            Exp::FunctionCall(name, args) | Exp::ImplicitFunctionCall(name, args) => {
                Self::push_used_var(name, used);
                for arg in args {
                    Self::collect_used_vars_exp(arg, used);
                }
            }
            Exp::IndirectCall(callee, args) => {
                Self::collect_used_vars_exp(callee, used);
                for arg in args {
                    Self::collect_used_vars_exp(arg, used);
                }
            }
            Exp::ArrayInit(elems) => {
                for elem in elems {
                    Self::collect_used_vars_exp(elem, used);
                }
            }
            Exp::DesignatedInit(designators, value) => {
                for designator in designators {
                    match designator {
                        Designator::Index(index) => Self::collect_used_vars_exp(index, used),
                        Designator::IndexRange(start, end) => {
                            Self::collect_used_vars_exp(start, used);
                            Self::collect_used_vars_exp(end, used);
                        }
                        Designator::Field(_) => {}
                    }
                }
                Self::collect_used_vars_exp(value, used);
            }
            Exp::AtomicFetch { ptr, arg, .. } => {
                Self::collect_used_vars_exp(ptr, used);
                Self::collect_used_vars_exp(arg, used);
            }
            Exp::AtomicExchange { ptr, value } => {
                Self::collect_used_vars_exp(ptr, used);
                Self::collect_used_vars_exp(value, used);
            }
            Exp::AtomicCompareExchange {
                ptr,
                expected,
                desired,
            }
            | Exp::AtomicCompareSwap {
                ptr,
                expected,
                desired,
                ..
            } => {
                Self::collect_used_vars_exp(ptr, used);
                Self::collect_used_vars_exp(expected, used);
                Self::collect_used_vars_exp(desired, used);
            }
            Exp::StatementExpr(block, result, _) => {
                Self::collect_used_vars_block(block, used);
                if let Some(result) = result {
                    Self::collect_used_vars_exp(result, used);
                }
            }
            _ => {}
        }
    }

    pub(super) fn collect_used_vars_stmt(stmt: &Statement, used: &mut Vec<String>) {
        match stmt {
            Statement::Return(Some(exp))
            | Statement::Expression(exp)
            | Statement::IndirectGoto(exp) => {
                Self::collect_used_vars_exp(exp, used);
            }
            Statement::If(cond, then_stmt, else_stmt) => {
                Self::collect_used_vars_exp(cond, used);
                Self::collect_used_vars_stmt(then_stmt, used);
                if let Some(else_stmt) = else_stmt {
                    Self::collect_used_vars_stmt(else_stmt, used);
                }
            }
            Statement::Block(block) => Self::collect_used_vars_block(block, used),
            Statement::While {
                condition, body, ..
            } => {
                Self::collect_used_vars_exp(condition, used);
                Self::collect_used_vars_stmt(body, used);
            }
            Statement::DoWhile {
                body, condition, ..
            } => {
                Self::collect_used_vars_stmt(body, used);
                Self::collect_used_vars_exp(condition, used);
            }
            Statement::For {
                init,
                condition,
                post,
                body,
                ..
            } => {
                match init.as_ref() {
                    ForInit::Declaration(vd) => {
                        if let Some(init) = vd.init.as_ref() {
                            Self::collect_used_vars_exp(init, used);
                        }
                    }
                    ForInit::Expression(Some(exp)) => Self::collect_used_vars_exp(exp, used),
                    ForInit::Expression(None) => {}
                }
                if let Some(condition) = condition {
                    Self::collect_used_vars_exp(condition, used);
                }
                if let Some(post) = post {
                    Self::collect_used_vars_exp(post, used);
                }
                Self::collect_used_vars_stmt(body, used);
            }
            Statement::Label(_, body)
            | Statement::Case { body, .. }
            | Statement::Default { body, .. } => Self::collect_used_vars_stmt(body, used),
            Statement::Switch { control, body, .. } => {
                Self::collect_used_vars_exp(control, used);
                Self::collect_used_vars_stmt(body, used);
            }
            _ => {}
        }
    }

    pub(super) fn rewrite_capture_block(
        block: Block,
        capture_map: &HashMap<String, String>,
    ) -> Block {
        block
            .into_iter()
            .map(|item| match item {
                BlockItem::Declaration(Declaration::VarDecl(mut vd)) => {
                    vd.init = vd
                        .init
                        .map(|init| Self::rewrite_capture_exp(init, capture_map));
                    BlockItem::Declaration(Declaration::VarDecl(vd))
                }
                BlockItem::Statement(stmt) => {
                    BlockItem::Statement(Self::rewrite_capture_stmt(stmt, capture_map))
                }
                other => other,
            })
            .collect()
    }

    pub(super) fn rewrite_capture_exp(exp: Exp, capture_map: &HashMap<String, String>) -> Exp {
        match exp {
            Exp::Var(name) => capture_map.get(&name).map_or(Exp::Var(name), |slot| {
                Exp::Unary(UnaryOp::Deref, Box::new(Exp::Var(slot.clone())))
            }),
            Exp::Cast(ct, ft, inner) => Exp::Cast(
                ct,
                ft,
                Box::new(Self::rewrite_capture_exp(*inner, capture_map)),
            ),
            Exp::Unary(op, inner) => {
                Exp::Unary(op, Box::new(Self::rewrite_capture_exp(*inner, capture_map)))
            }
            Exp::Binary(op, left, right) => Exp::Binary(
                op,
                Box::new(Self::rewrite_capture_exp(*left, capture_map)),
                Box::new(Self::rewrite_capture_exp(*right, capture_map)),
            ),
            Exp::Assign(left, right) => Exp::Assign(
                Box::new(Self::rewrite_capture_exp(*left, capture_map)),
                Box::new(Self::rewrite_capture_exp(*right, capture_map)),
            ),
            Exp::CompoundAssign(op, left, right) => Exp::CompoundAssign(
                op,
                Box::new(Self::rewrite_capture_exp(*left, capture_map)),
                Box::new(Self::rewrite_capture_exp(*right, capture_map)),
            ),
            Exp::Conditional(a, b, c) => Exp::Conditional(
                Box::new(Self::rewrite_capture_exp(*a, capture_map)),
                Box::new(Self::rewrite_capture_exp(*b, capture_map)),
                Box::new(Self::rewrite_capture_exp(*c, capture_map)),
            ),
            Exp::BuiltinExpect(value, hints) => Exp::BuiltinExpect(
                Box::new(Self::rewrite_capture_exp(*value, capture_map)),
                hints
                    .into_iter()
                    .map(|arg| Self::rewrite_capture_exp(arg, capture_map))
                    .collect(),
            ),
            Exp::FunctionCall(name, args) => Exp::FunctionCall(
                name,
                args.into_iter()
                    .map(|arg| Self::rewrite_capture_exp(arg, capture_map))
                    .collect(),
            ),
            Exp::ImplicitFunctionCall(name, args) => Exp::ImplicitFunctionCall(
                name,
                args.into_iter()
                    .map(|arg| Self::rewrite_capture_exp(arg, capture_map))
                    .collect(),
            ),
            Exp::Subscript(a, b) => Exp::Subscript(
                Box::new(Self::rewrite_capture_exp(*a, capture_map)),
                Box::new(Self::rewrite_capture_exp(*b, capture_map)),
            ),
            Exp::ArrayInit(elems) => Exp::ArrayInit(
                elems
                    .into_iter()
                    .map(|elem| Self::rewrite_capture_exp(elem, capture_map))
                    .collect(),
            ),
            Exp::DesignatedInit(designators, value) => Exp::DesignatedInit(
                designators
                    .into_iter()
                    .map(|designator| match designator {
                        Designator::Index(index) => Designator::Index(Box::new(
                            Self::rewrite_capture_exp(*index, capture_map),
                        )),
                        Designator::IndexRange(start, end) => Designator::IndexRange(
                            Box::new(Self::rewrite_capture_exp(*start, capture_map)),
                            Box::new(Self::rewrite_capture_exp(*end, capture_map)),
                        ),
                        other => other,
                    })
                    .collect(),
                Box::new(Self::rewrite_capture_exp(*value, capture_map)),
            ),
            Exp::Dot(inner, member) => Exp::Dot(
                Box::new(Self::rewrite_capture_exp(*inner, capture_map)),
                member,
            ),
            Exp::Arrow(inner, member) => Exp::Arrow(
                Box::new(Self::rewrite_capture_exp(*inner, capture_map)),
                member,
            ),
            Exp::Comma(left, right) => Exp::Comma(
                Box::new(Self::rewrite_capture_exp(*left, capture_map)),
                Box::new(Self::rewrite_capture_exp(*right, capture_map)),
            ),
            Exp::StatementExpr(block, result, ft) => Exp::StatementExpr(
                Self::rewrite_capture_block(block, capture_map),
                result.map(|result| Box::new(Self::rewrite_capture_exp(*result, capture_map))),
                ft,
            ),
            Exp::IndirectCall(callee, args) => Exp::IndirectCall(
                Box::new(Self::rewrite_capture_exp(*callee, capture_map)),
                args.into_iter()
                    .map(|arg| Self::rewrite_capture_exp(arg, capture_map))
                    .collect(),
            ),
            other => other,
        }
    }

    pub(super) fn rewrite_capture_stmt(
        stmt: Statement,
        capture_map: &HashMap<String, String>,
    ) -> Statement {
        match stmt {
            Statement::Return(exp) => {
                Statement::Return(exp.map(|exp| Self::rewrite_capture_exp(exp, capture_map)))
            }
            Statement::Expression(exp) => {
                Statement::Expression(Self::rewrite_capture_exp(exp, capture_map))
            }
            Statement::IndirectGoto(exp) => {
                Statement::IndirectGoto(Self::rewrite_capture_exp(exp, capture_map))
            }
            Statement::If(cond, then_stmt, else_stmt) => Statement::If(
                Self::rewrite_capture_exp(cond, capture_map),
                Box::new(Self::rewrite_capture_stmt(*then_stmt, capture_map)),
                else_stmt.map(|stmt| Box::new(Self::rewrite_capture_stmt(*stmt, capture_map))),
            ),
            Statement::Block(block) => {
                Statement::Block(Self::rewrite_capture_block(block, capture_map))
            }
            Statement::While {
                condition,
                body,
                label,
            } => Statement::While {
                condition: Self::rewrite_capture_exp(condition, capture_map),
                body: Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
                label,
            },
            Statement::DoWhile {
                body,
                condition,
                label,
            } => Statement::DoWhile {
                body: Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
                condition: Self::rewrite_capture_exp(condition, capture_map),
                label,
            },
            Statement::For {
                init,
                condition,
                post,
                body,
                label,
            } => Statement::For {
                init: Box::new(match *init {
                    ForInit::Declaration(mut vd) => {
                        vd.init = vd
                            .init
                            .map(|init| Self::rewrite_capture_exp(init, capture_map));
                        ForInit::Declaration(vd)
                    }
                    ForInit::Expression(exp) => ForInit::Expression(
                        exp.map(|exp| Self::rewrite_capture_exp(exp, capture_map)),
                    ),
                }),
                condition: condition.map(|exp| Self::rewrite_capture_exp(exp, capture_map)),
                post: post.map(|exp| Self::rewrite_capture_exp(exp, capture_map)),
                body: Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
                label,
            },
            Statement::Label(name, body) => Statement::Label(
                name,
                Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
            ),
            Statement::Switch {
                control,
                body,
                label,
                cases,
            } => Statement::Switch {
                control: Self::rewrite_capture_exp(control, capture_map),
                body: Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
                label,
                cases,
            },
            Statement::Case {
                value,
                end_value,
                body,
                label,
            } => Statement::Case {
                value: Self::rewrite_capture_exp(value, capture_map),
                end_value: end_value
                    .map(|end_value| Self::rewrite_capture_exp(end_value, capture_map)),
                body: Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
                label,
            },
            Statement::Default { body, label } => Statement::Default {
                body: Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
                label,
            },
            other => other,
        }
    }

    pub(super) fn collect_statement_labels(stmt: &Statement, labels: &mut HashSet<String>) {
        match stmt {
            Statement::Label(name, body) => {
                labels.insert(name.clone());
                Self::collect_statement_labels(body, labels);
            }
            Statement::Block(block) => Self::collect_block_labels(block, labels),
            Statement::If(_, then_stmt, else_stmt) => {
                Self::collect_statement_labels(then_stmt, labels);
                if let Some(else_stmt) = else_stmt {
                    Self::collect_statement_labels(else_stmt, labels);
                }
            }
            Statement::While { body, .. }
            | Statement::DoWhile { body, .. }
            | Statement::For { body, .. }
            | Statement::Switch { body, .. }
            | Statement::Case { body, .. }
            | Statement::Default { body, .. } => Self::collect_statement_labels(body, labels),
            Statement::Return(_)
            | Statement::Expression(_)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Goto(_)
            | Statement::IndirectGoto(_)
            | Statement::Null => {}
        }
    }

    pub(super) fn collect_statement_label_bodies(
        stmt: &Statement,
        labels: &mut IndexMap<String, Statement>,
    ) {
        match stmt {
            Statement::Label(name, body) => {
                labels.insert(name.clone(), body.as_ref().clone());
                Self::collect_statement_label_bodies(body, labels);
            }
            Statement::Block(block) => Self::collect_block_label_bodies(block, labels),
            Statement::If(_, then_stmt, else_stmt) => {
                Self::collect_statement_label_bodies(then_stmt, labels);
                if let Some(else_stmt) = else_stmt {
                    Self::collect_statement_label_bodies(else_stmt, labels);
                }
            }
            Statement::While { body, .. }
            | Statement::DoWhile { body, .. }
            | Statement::For { body, .. }
            | Statement::Switch { body, .. }
            | Statement::Case { body, .. }
            | Statement::Default { body, .. } => Self::collect_statement_label_bodies(body, labels),
            Statement::Return(_)
            | Statement::Expression(_)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Goto(_)
            | Statement::IndirectGoto(_)
            | Statement::Null => {}
        }
    }

    pub(super) fn collect_block_labels(block: &Block, labels: &mut HashSet<String>) {
        for item in block {
            if let BlockItem::Statement(stmt) = item {
                Self::collect_statement_labels(stmt, labels);
            }
        }
    }

    pub(super) fn collect_block_label_bodies(
        block: &Block,
        labels: &mut IndexMap<String, Statement>,
    ) {
        for item in block {
            if let BlockItem::Statement(stmt) = item {
                Self::collect_statement_label_bodies(stmt, labels);
            }
        }
    }

    pub(super) fn collect_parent_label_gotos_stmt(
        stmt: &Statement,
        local_labels: &HashSet<String>,
        parent_labels: &HashSet<String>,
    ) -> HashSet<String> {
        let mut labels = HashSet::new();
        Self::collect_parent_label_gotos_stmt_into(stmt, local_labels, parent_labels, &mut labels);
        labels
    }

    pub(super) fn collect_parent_label_gotos_stmt_into(
        stmt: &Statement,
        local_labels: &HashSet<String>,
        parent_labels: &HashSet<String>,
        labels: &mut HashSet<String>,
    ) {
        match stmt {
            Statement::Goto(label)
                if !local_labels.contains(label) && parent_labels.contains(label) =>
            {
                labels.insert(label.clone());
            }
            Statement::If(_, then_stmt, else_stmt) => {
                Self::collect_parent_label_gotos_stmt_into(
                    then_stmt,
                    local_labels,
                    parent_labels,
                    labels,
                );
                if let Some(else_stmt) = else_stmt {
                    Self::collect_parent_label_gotos_stmt_into(
                        else_stmt,
                        local_labels,
                        parent_labels,
                        labels,
                    );
                }
            }
            Statement::Block(block) => {
                for item in block {
                    if let BlockItem::Statement(stmt) = item {
                        Self::collect_parent_label_gotos_stmt_into(
                            stmt,
                            local_labels,
                            parent_labels,
                            labels,
                        );
                    }
                }
            }
            Statement::While { body, .. }
            | Statement::DoWhile { body, .. }
            | Statement::For { body, .. }
            | Statement::Label(_, body)
            | Statement::Switch { body, .. }
            | Statement::Case { body, .. }
            | Statement::Default { body, .. } => Self::collect_parent_label_gotos_stmt_into(
                body,
                local_labels,
                parent_labels,
                labels,
            ),
            Statement::Return(_)
            | Statement::Expression(_)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Goto(_)
            | Statement::IndirectGoto(_)
            | Statement::Null => {}
        }
    }

    pub(super) fn collect_nested_parent_label_gotos_block(
        block: &Block,
        parent_labels: &HashSet<String>,
    ) -> HashSet<String> {
        let mut labels = HashSet::new();
        for item in block {
            match item {
                BlockItem::Declaration(Declaration::FunDecl(fd)) => {
                    if let Some(body) = fd.body.as_ref() {
                        let mut local_labels = HashSet::new();
                        Self::collect_block_labels(body, &mut local_labels);
                        labels.extend(Self::collect_parent_label_gotos_stmt(
                            &Statement::Block(body.clone()),
                            &local_labels,
                            parent_labels,
                        ));
                    }
                }
                BlockItem::Statement(stmt) => {
                    Self::collect_nested_parent_label_gotos_stmt(stmt, parent_labels, &mut labels);
                }
                _ => {}
            }
        }
        labels
    }

    pub(super) fn collect_nested_parent_label_gotos_stmt(
        stmt: &Statement,
        parent_labels: &HashSet<String>,
        labels: &mut HashSet<String>,
    ) {
        match stmt {
            Statement::Block(block) => {
                labels.extend(Self::collect_nested_parent_label_gotos_block(
                    block,
                    parent_labels,
                ));
            }
            Statement::If(_, then_stmt, else_stmt) => {
                Self::collect_nested_parent_label_gotos_stmt(then_stmt, parent_labels, labels);
                if let Some(else_stmt) = else_stmt {
                    Self::collect_nested_parent_label_gotos_stmt(else_stmt, parent_labels, labels);
                }
            }
            Statement::While { body, .. }
            | Statement::DoWhile { body, .. }
            | Statement::For { body, .. }
            | Statement::Label(_, body)
            | Statement::Switch { body, .. }
            | Statement::Case { body, .. }
            | Statement::Default { body, .. } => {
                Self::collect_nested_parent_label_gotos_stmt(body, parent_labels, labels)
            }
            Statement::Return(_)
            | Statement::Expression(_)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Goto(_)
            | Statement::IndirectGoto(_)
            | Statement::Null => {}
        }
    }

    pub(super) fn collect_escaped_function_refs_exp(exp: &Exp, refs: &mut HashSet<String>) {
        match exp {
            Exp::Var(name) => {
                refs.insert(name.clone());
            }
            Exp::FunctionCall(_, args) | Exp::ImplicitFunctionCall(_, args) => {
                for arg in args {
                    Self::collect_escaped_function_refs_exp(arg, refs);
                }
            }
            Exp::Cast(_, _, inner)
            | Exp::Unary(_, inner)
            | Exp::SizeOf(inner)
            | Exp::Dot(inner, _)
            | Exp::Arrow(inner, _) => Self::collect_escaped_function_refs_exp(inner, refs),
            Exp::Binary(_, left, right)
            | Exp::Assign(left, right)
            | Exp::CompoundAssign(_, left, right)
            | Exp::Subscript(left, right)
            | Exp::Comma(left, right) => {
                Self::collect_escaped_function_refs_exp(left, refs);
                Self::collect_escaped_function_refs_exp(right, refs);
            }
            Exp::Conditional(cond, then_exp, else_exp) => {
                Self::collect_escaped_function_refs_exp(cond, refs);
                Self::collect_escaped_function_refs_exp(then_exp, refs);
                Self::collect_escaped_function_refs_exp(else_exp, refs);
            }
            Exp::BuiltinExpect(value, hints) => {
                Self::collect_escaped_function_refs_exp(value, refs);
                for hint in hints {
                    Self::collect_escaped_function_refs_exp(hint, refs);
                }
            }
            Exp::ArrayInit(elems) => {
                for elem in elems {
                    Self::collect_escaped_function_refs_exp(elem, refs);
                }
            }
            Exp::DesignatedInit(_, value) => Self::collect_escaped_function_refs_exp(value, refs),
            Exp::StatementExpr(block, tail, _) => {
                Self::collect_escaped_function_refs_block(block, refs);
                if let Some(tail) = tail {
                    Self::collect_escaped_function_refs_exp(tail, refs);
                }
            }
            Exp::IndirectCall(callee, args) => {
                Self::collect_escaped_function_refs_exp(callee, refs);
                for arg in args {
                    Self::collect_escaped_function_refs_exp(arg, refs);
                }
            }
            Exp::AtomicFetch { ptr, arg, .. } => {
                Self::collect_escaped_function_refs_exp(ptr, refs);
                Self::collect_escaped_function_refs_exp(arg, refs);
            }
            Exp::AtomicExchange { ptr, value } => {
                Self::collect_escaped_function_refs_exp(ptr, refs);
                Self::collect_escaped_function_refs_exp(value, refs);
            }
            Exp::AtomicCompareExchange {
                ptr,
                expected,
                desired,
            }
            | Exp::AtomicCompareSwap {
                ptr,
                expected,
                desired,
                ..
            } => {
                Self::collect_escaped_function_refs_exp(ptr, refs);
                Self::collect_escaped_function_refs_exp(expected, refs);
                Self::collect_escaped_function_refs_exp(desired, refs);
            }
            Exp::Constant(_)
            | Exp::LongConstant(_)
            | Exp::Int128Constant(_)
            | Exp::UIntConstant(_)
            | Exp::ULongConstant(_)
            | Exp::UInt128Constant(_)
            | Exp::DoubleConstant(_)
            | Exp::LongDoubleConstant(_)
            | Exp::ImaginaryIntConstant(_)
            | Exp::ImaginaryDoubleConstant(_)
            | Exp::StringLiteral(_)
            | Exp::WideStringLiteral(_)
            | Exp::Utf16StringLiteral(_)
            | Exp::Utf32StringLiteral(_)
            | Exp::LabelAddress(_)
            | Exp::SizeOfType(_, _)
            | Exp::AlignOfType(_)
            | Exp::Unreachable
            | Exp::AtomicFence => {}
        }
    }

    pub(super) fn collect_escaped_function_refs_stmt(stmt: &Statement, refs: &mut HashSet<String>) {
        match stmt {
            Statement::Return(Some(exp))
            | Statement::Expression(exp)
            | Statement::IndirectGoto(exp) => Self::collect_escaped_function_refs_exp(exp, refs),
            Statement::Return(None)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Goto(_)
            | Statement::Null => {}
            Statement::If(cond, then_stmt, else_stmt) => {
                Self::collect_escaped_function_refs_exp(cond, refs);
                Self::collect_escaped_function_refs_stmt(then_stmt, refs);
                if let Some(else_stmt) = else_stmt {
                    Self::collect_escaped_function_refs_stmt(else_stmt, refs);
                }
            }
            Statement::Block(block) => Self::collect_escaped_function_refs_block(block, refs),
            Statement::While {
                condition, body, ..
            }
            | Statement::DoWhile {
                condition, body, ..
            } => {
                Self::collect_escaped_function_refs_exp(condition, refs);
                Self::collect_escaped_function_refs_stmt(body, refs);
            }
            Statement::For {
                init,
                condition,
                post,
                body,
                ..
            } => {
                if let ForInit::Expression(Some(exp)) = init.as_ref() {
                    Self::collect_escaped_function_refs_exp(exp, refs);
                }
                if let Some(condition) = condition {
                    Self::collect_escaped_function_refs_exp(condition, refs);
                }
                if let Some(post) = post {
                    Self::collect_escaped_function_refs_exp(post, refs);
                }
                Self::collect_escaped_function_refs_stmt(body, refs);
            }
            Statement::Label(_, body)
            | Statement::Case { body, .. }
            | Statement::Default { body, .. } => {
                Self::collect_escaped_function_refs_stmt(body, refs)
            }
            Statement::Switch { control, body, .. } => {
                Self::collect_escaped_function_refs_exp(control, refs);
                Self::collect_escaped_function_refs_stmt(body, refs);
            }
        }
    }

    pub(super) fn collect_escaped_function_refs_block(block: &Block, refs: &mut HashSet<String>) {
        for item in block {
            match item {
                BlockItem::Declaration(Declaration::VarDecl(vd)) => {
                    if let Some(init) = vd.init.as_ref() {
                        Self::collect_escaped_function_refs_exp(init, refs);
                    }
                }
                BlockItem::Statement(stmt) => Self::collect_escaped_function_refs_stmt(stmt, refs),
                _ => {}
            }
        }
    }

    pub(super) fn jump_env_full_type() -> FullType {
        FullType::Array {
            elem: Box::new(FullType::Scalar(CType::Char)),
            size: 24,
        }
    }

    pub(super) fn prepare_nonlocal_label_envs(
        &mut self,
        labels: &HashSet<String>,
    ) -> HashMap<String, String> {
        let mut envs = HashMap::new();
        let mut sorted_labels: Vec<_> = labels.iter().cloned().collect();
        sorted_labels.sort();
        for label in sorted_labels {
            let env = format!(
                "__rnqcc_nonlocal_label_env_{}_{}",
                self.current_function,
                label.replace('.', "_")
            );
            self.register_var(&env, Self::jump_env_full_type());
            self.current_function_locals.insert(env.clone());
            envs.insert(label, env);
        }
        envs
    }

    pub(super) fn emit_nonlocal_label_env_setup(&mut self) {
        let mut envs: Vec<_> = self
            .current_nonlocal_label_envs
            .iter()
            .map(|(label, env)| (label.clone(), env.clone()))
            .collect();
        envs.sort_by(|left, right| left.0.cmp(&right.0));
        for (label, env) in envs {
            let buf = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::GetAddress {
                src: TackyVal::Var(env),
                dst: buf.clone(),
            });
            let jumped = self.fresh_tmp(CType::Int);
            let resume_label = self.fresh_label("nonlocal_label_resume");
            let end_label = self.fresh_label("nonlocal_label_setjmp_end");
            self.emit(TackyInstr::BuiltinSetjmp {
                buf,
                dst: jumped.clone(),
                label: resume_label,
                end_label,
            });
            let normal_label = self.fresh_label("nonlocal_label_normal");
            self.emit(TackyInstr::JumpIfZero(jumped, normal_label.clone()));
            self.emit(TackyInstr::Jump(format!(
                "label.{}.{}",
                self.current_function, label
            )));
            self.emit(TackyInstr::Label(normal_label));
        }
    }

    pub(super) fn emit_function(
        &mut self,
        func: FunctionDeclaration,
    ) -> TackyResult<Option<TackyFunction>> {
        self.function_symbols.insert(func.name.clone());
        let param_types: Vec<CType> = func.params.iter().map(|(_, t, _)| *t).collect();
        self.func_types.insert(
            func.name.clone(),
            (
                func.return_type,
                param_types,
                func.return_ptr_info,
                func.variadic,
            ),
        );
        if func.old_style {
            self.old_style_functions.insert(func.name.clone());
        } else {
            self.old_style_functions.remove(&func.name);
        }
        self.record_zero_fixed_variadic_function(&func);
        self.func_param_full_types
            .insert(func.name.clone(), func.param_full_types.clone());
        if let Some(ref rft) = func.return_full_type {
            self.func_full_types.insert(func.name.clone(), rft.clone());
        }

        let Some(body) = func.body else {
            return Ok(None);
        };

        self.current_function = func.name.clone();
        self.current_function_params = func
            .params
            .iter()
            .map(|(name, _, _)| name.clone())
            .collect();
        self.current_function_locals = self.current_function_params.iter().cloned().collect();
        Self::collect_declared_names(&body, &mut self.current_function_locals);
        self.instructions.clear();
        let mut local_labels = HashSet::new();
        Self::collect_block_labels(&body, &mut local_labels);
        let nonlocal_label_targets =
            Self::collect_nested_parent_label_gotos_block(&body, &local_labels);
        let nonlocal_label_envs = self.prepare_nonlocal_label_envs(&nonlocal_label_targets);
        let saved_nonlocal_label_envs =
            std::mem::replace(&mut self.current_nonlocal_label_envs, nonlocal_label_envs);
        let saved_label_bodies = std::mem::take(&mut self.current_label_bodies);
        Self::collect_block_label_bodies(&body, &mut self.current_label_bodies);
        let saved_escaped_functions = std::mem::take(&mut self.current_escaped_functions);
        Self::collect_escaped_function_refs_block(&body, &mut self.current_escaped_functions);
        self.local_label_stack.push(local_labels);
        let saved_deprecated_vars = std::mem::take(&mut self.deprecated_vars);
        let saved_warned_deprecated_vars = std::mem::take(&mut self.warned_deprecated_vars);
        self.deprecated_vars.extend(
            func.deprecated_params
                .iter()
                .map(|param| (param.name.clone(), param.message.clone())),
        );

        // Check if return type requires hidden pointer
        let ret_needs_hidden_ptr = if let Some(ref ret_ft) = func.return_full_type {
            let needs_large_struct_ptr = if let FullType::Struct(tag) = ret_ft {
                self.struct_defs
                    .get(tag)
                    .map(|d| d.size > 16)
                    .unwrap_or(false)
            } else {
                false
            };
            needs_large_struct_ptr || ret_ft.is_complex()
        } else {
            false
        };
        self.hidden_ret_ptr = if ret_needs_hidden_ptr {
            let name = format!("__ret_ptr_{}", func.name);
            self.var_types.insert(name.clone(), CType::Pointer);
            self.symbol_types.insert(name.clone(), CType::Pointer);
            Some(name)
        } else {
            None
        };
        let hidden_ret_ptr_name = self.hidden_ret_ptr.clone();

        // Register params — decompose struct params into eightbytes
        let mut tacky_params = Vec::new();
        let mut stack_params = std::collections::HashSet::new();
        let mut memory_param_blocks = Vec::new();
        let mut struct_param_groups: Vec<(usize, usize, Vec<bool>)> = Vec::new();
        let mut complex_param_fixups: Vec<(String, usize)> = Vec::new();
        if let Some(ref ret_ptr) = hidden_ret_ptr_name {
            tacky_params.push(ret_ptr.clone());
        }
        let mut struct_param_fixups: Vec<(String, String, StructDef)> = Vec::new(); // (original_name, tag, def)
        let mut param_vla_bounds = func.param_vla_bounds.iter();
        for (i, (name, ptype, pi)) in func.params.iter().enumerate() {
            let ft = if i < func.param_full_types.len() {
                func.param_full_types[i].clone()
            } else {
                FullType::from_decl(*ptype, *pi, &None)
            };
            if let FullType::Pointer(pointee) = &ft {
                if let FullType::Array { elem, size } = pointee.as_ref() {
                    if *size == VLA_STATIC_SCALE_FALLBACK {
                        if let Some(bound) = param_vla_bounds.next() {
                            self.vla_param_bounds.insert(
                                name.clone(),
                                Exp::Binary(
                                    BinaryOp::Mul,
                                    Box::new(bound.clone()),
                                    Box::new(Exp::SizeOfType(
                                        elem.to_ctype(),
                                        elem.as_ref().clone(),
                                    )),
                                ),
                            );
                        }
                    }
                }
            }

            if let FullType::Struct(ref tag) = ft {
                if let Some(def) = self.struct_defs.get(tag).cloned() {
                    let classes = def.classify_with(&self.struct_defs);
                    if classes.len() == 1 && classes[0] == ParamClass::Memory {
                        let param_name = format!("{}_mem", name);
                        let param_idx = tacky_params.len();
                        self.var_types.insert(param_name.clone(), CType::Pointer);
                        self.symbol_types.insert(param_name.clone(), CType::Pointer);
                        tacky_params.push(param_name.clone());
                        stack_params.insert(param_name);
                        memory_param_blocks.push((param_idx, name.clone(), def.size));
                    } else {
                        // Decompose into eightbyte params
                        let group_start = tacky_params.len();
                        let is_sse_vec: Vec<bool> =
                            classes.iter().map(|c| *c == ParamClass::Sse).collect();
                        for (eb_idx, class) in classes.iter().enumerate() {
                            let param_name = format!("{}_eb{}", name, eb_idx);
                            let param_type = match class {
                                ParamClass::Sse => CType::Double,
                                _ => CType::Long, // eightbytes always use full 64-bit register
                            };
                            self.var_types.insert(param_name.clone(), param_type);
                            self.symbol_types.insert(param_name.clone(), param_type);
                            tacky_params.push(param_name);
                        }
                        if !classes.is_empty() {
                            struct_param_groups.push((group_start, classes.len(), is_sse_vec));
                        }
                    }
                    // Register the original struct var — allocate enough for eightbyte storage
                    let classes = def.classify_with(&self.struct_defs);
                    let alloc_size = std::cmp::max(def.size, classes.len() * 8);
                    struct_param_fixups.push((name.clone(), tag.clone(), def));
                    self.register_var(name, ft.clone());
                    self.array_sizes.insert(name.clone(), alloc_size);
                    continue;
                }
            }
            if ft.is_complex() {
                let FullType::Vector { elem, .. } = &ft else {
                    return Err("internal error: expected complex vector type".to_string());
                };
                let elem_type = elem.to_ctype();
                let elem_size = elem.byte_size_with(&self.struct_defs);
                let group_start = tacky_params.len();
                let flat_names = [format!("{}_eb0", name), format!("{}_eb1", name)];
                for flat_name in &flat_names {
                    self.var_types.insert(flat_name.clone(), elem_type);
                    self.symbol_types.insert(flat_name.clone(), elem_type);
                    tacky_params.push(flat_name.clone());
                }
                let is_fp = elem_type.is_floating();
                struct_param_groups.push((group_start, 2, vec![is_fp, is_fp]));
                complex_param_fixups.push((name.clone(), elem_size));
                self.register_var(name, ft.clone());
                self.array_sizes
                    .insert(name.clone(), ft.byte_size_with(&self.struct_defs));
                continue;
            }

            let storage_type = self.storage_ctype_for_full(&ft);
            self.var_types.insert(name.clone(), storage_type);
            self.symbol_types.insert(name.clone(), storage_type);
            if let Some(info) = pi {
                self.ptr_info.insert(name.clone(), *info);
            }
            if ft.is_vector() {
                self.array_sizes
                    .insert(name.clone(), ft.byte_size_with(&self.struct_defs));
            }
            self.full_types.insert(name.clone(), ft);
            tacky_params.push(name.clone());
        }

        // Reassemble struct params from eightbytes
        for (name, _tag, def) in &struct_param_fixups {
            let classes = def.classify_with(&self.struct_defs);
            if classes.len() == 1 && classes[0] == ParamClass::Memory {
                continue;
            }
            let num_ebs = if classes.len() == 1 && classes[0] == ParamClass::Memory {
                def.size.div_ceil(8)
            } else {
                classes.len()
            };
            for eb_idx in 0..num_ebs {
                let param_name = format!("{}_eb{}", name, eb_idx);
                let eb_offset = (eb_idx * 8) as i64;
                self.emit(TackyInstr::CopyToOffset {
                    src: TackyVal::Var(param_name),
                    dst_name: name.clone(),
                    offset: eb_offset,
                });
            }
        }

        for (name, elem_size) in &complex_param_fixups {
            for eb_idx in 0..2 {
                let param_name = format!("{}_eb{}", name, eb_idx);
                let eb_offset = (eb_idx * *elem_size) as i64;
                self.emit(TackyInstr::CopyToOffset {
                    src: TackyVal::Var(param_name),
                    dst_name: name.clone(),
                    offset: eb_offset,
                });
            }
        }

        self.emit_nonlocal_label_env_setup();
        let emit_result = self.emit_block(body);
        self.local_label_stack.pop();
        self.current_label_bodies = saved_label_bodies;
        self.current_escaped_functions = saved_escaped_functions;
        self.current_nonlocal_label_envs = saved_nonlocal_label_envs;
        self.deprecated_vars = saved_deprecated_vars;
        self.warned_deprecated_vars = saved_warned_deprecated_vars;
        emit_result?;
        self.emit(TackyInstr::Return(TackyVal::Constant(0)));
        self.apply_function_instrumentation(func.no_instrument_function);

        Ok(Some(TackyFunction {
            name: func.name,
            return_type: func.return_type,
            params: tacky_params,
            global: true, // overridden by linkage map in generate()
            body: std::mem::take(&mut self.instructions),
            stack_params,
            memory_param_blocks,
            struct_param_groups,
        }))
    }
}
