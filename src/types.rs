// ============================================================
// Target & Compiler Stage (re-exported from target module)
// ============================================================
pub use crate::target::*;

use indexmap::IndexMap;

/// Internal placeholder used for a nonconstant array bound.
///
/// This value is deliberately outside the small range used by normal object
/// layouts.  It must not overlap a valid fixed dimension such as `[16]`.
pub const VLA_STATIC_SCALE_FALLBACK: usize = usize::MAX - 1;

const VLA_SIZE_ESTIMATE: usize = 16;

fn array_size_for_layout(size: usize) -> usize {
    if size == VLA_STATIC_SCALE_FALLBACK {
        VLA_SIZE_ESTIMATE
    } else {
        size
    }
}

#[derive(Debug, PartialEq)]
pub enum Stage {
    Preprocess,
    Lex,
    Parse,
    Validate,
    Tacky,
    Codegen,
    Assembly,
    Object,
    Executable,
}

impl Stage {
    pub const NAMES: [&'static str; 6] = ["lex", "parse", "validate", "tacky", "codegen", "s"];

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "lex" => Some(Stage::Lex),
            "parse" => Some(Stage::Parse),
            "validate" => Some(Stage::Validate),
            "tacky" => Some(Stage::Tacky),
            "codegen" => Some(Stage::Codegen),
            "s" => Some(Stage::Assembly),
            _ => None,
        }
    }

    pub fn accepts_output(&self) -> bool {
        matches!(
            self,
            Stage::Preprocess | Stage::Assembly | Stage::Object | Stage::Executable
        )
    }

    pub fn output_requires_single_input(&self) -> bool {
        matches!(self, Stage::Preprocess | Stage::Assembly | Stage::Object)
    }
}

// ============================================================
// C Types
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CType {
    Char,
    SChar,
    UChar,
    Short,
    UShort,
    Int,
    Long,
    Int128,
    UInt,
    ULong,
    UInt128,
    Float,
    Double,
    LongDouble,
    Bool,
    /// Struct type (tag tracked separately via FullType)
    Struct,
    /// Pointer to some type. We don't track the pointee type at the assembly level —
    /// all pointers are 8 bytes. The pointee type is only needed for type checking
    /// which we handle during parsing/TACKY generation.
    Pointer,
    Void,
}

pub fn c_string_bytes(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let value = c as u32;
        if value <= u8::MAX as u32 {
            out.push(value as u8);
        } else {
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}

pub fn c_string_byte_len(s: &str) -> usize {
    s.chars()
        .map(|c| {
            if (c as u32) <= u8::MAX as u32 {
                1
            } else {
                c.len_utf8()
            }
        })
        .sum()
}

pub fn c_string_contains_zero(s: &str) -> bool {
    s.as_bytes().contains(&0)
}

pub fn c_string_truncate_bytes(s: &str, max_bytes: usize) -> String {
    let mut out = String::with_capacity(max_bytes.min(s.len()));
    let mut used = 0usize;
    for c in s.chars() {
        let width = if (c as u32) <= u8::MAX as u32 {
            1
        } else {
            c.len_utf8()
        };
        if used + width > max_bytes {
            break;
        }
        out.push(c);
        used += width;
    }
    out
}

impl CType {
    pub fn size(self) -> i32 {
        match self {
            CType::Char | CType::SChar | CType::UChar | CType::Bool => 1,
            CType::Short | CType::UShort => 2,
            CType::Int | CType::UInt | CType::Float => 4,
            CType::Long | CType::ULong | CType::Double | CType::Pointer => 8,
            CType::Int128 | CType::UInt128 | CType::LongDouble => 16,
            CType::Void => 0,
            CType::Struct => 0, // size tracked via FullType/StructDef
        }
    }

    pub fn is_signed(self) -> bool {
        matches!(
            self,
            CType::Char | CType::SChar | CType::Short | CType::Int | CType::Long | CType::Int128
        )
    }

    pub fn is_char(self) -> bool {
        matches!(self, CType::Char | CType::SChar | CType::UChar)
    }

    pub fn is_small_integer(self) -> bool {
        matches!(
            self,
            CType::Char | CType::SChar | CType::UChar | CType::Short | CType::UShort | CType::Bool
        )
    }

    pub fn is_struct(self) -> bool {
        self == CType::Struct
    }

    pub fn is_double(self) -> bool {
        matches!(self, CType::Double | CType::LongDouble)
    }

    pub fn is_floating(self) -> bool {
        matches!(self, CType::Float | CType::Double | CType::LongDouble)
    }

    pub fn is_pointer(self) -> bool {
        self == CType::Pointer
    }

    /// Integer promotion: char types promote to Int
    pub fn promote(self) -> CType {
        if self.is_small_integer() {
            CType::Int
        } else {
            self
        }
    }

    /// Usual arithmetic conversions (C standard 6.3.1.8)
    pub fn common(a: CType, b: CType) -> CType {
        // Integer promotions first
        let a = a.promote();
        let b = b.promote();
        if a == b {
            return a;
        }
        if a == CType::LongDouble || b == CType::LongDouble {
            return CType::LongDouble;
        }
        if a == CType::Double {
            return CType::Double;
        }
        if b == CType::Double {
            return CType::Double;
        }
        if a == CType::Float {
            return CType::Float;
        }
        if b == CType::Float {
            return CType::Float;
        }
        if a == CType::Pointer {
            return CType::Pointer;
        }
        if b == CType::Pointer {
            return CType::Pointer;
        }
        if a.size() == b.size() {
            if a.is_signed() {
                return b;
            } else {
                return a;
            }
        }
        if a.size() > b.size() {
            a
        } else {
            b
        }
    }
}

pub type PtrInfo = (CType, usize);
pub type ParamDecl = (String, CType, Option<PtrInfo>);
pub type FunctionTypeInfo = (CType, Vec<CType>, Option<PtrInfo>, bool);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeprecatedParam {
    pub name: String,
    pub message: Option<String>,
}

// ============================================================
// Full Type (rich type representation for type checking)
// ============================================================

/// Rich type that tracks array dimensions and pointer targets.
/// Used in TACKY generation for type checking. CType remains for codegen.
#[derive(Debug, Clone, PartialEq)]
pub enum FullType {
    Scalar(CType),
    Pointer(Box<FullType>),
    Function {
        return_type: Box<FullType>,
        params: Vec<FullType>,
        variadic: bool,
    },
    Array {
        elem: Box<FullType>,
        size: usize,
    },
    Vector {
        elem: Box<FullType>,
        lanes: usize,
        complex: bool,
    },
    Struct(String), // struct tag name (resolved to unique identifier)
}

impl FullType {
    /// Convert to CType for codegen (arrays become ByteArray info, pointers become Pointer)
    pub fn to_ctype(&self) -> CType {
        match self {
            FullType::Scalar(t) => *t,
            FullType::Pointer(_) => CType::Pointer,
            FullType::Function { .. } => CType::Pointer,
            FullType::Array { .. } => CType::Pointer, // arrays decay to pointers in most contexts
            FullType::Vector { elem, .. } => elem.to_ctype(),
            FullType::Struct(_) => CType::Struct,
        }
    }

    /// Total byte size of this type (note: for Struct, returns 0 without
    /// struct_defs). This compatibility API saturates on oversized arrays or
    /// vectors; callers that need to diagnose overflow should use
    /// [`FullType::checked_byte_size_with`].
    pub fn byte_size(&self) -> usize {
        match self {
            FullType::Scalar(t) => std::cmp::max(t.size() as usize, 1),
            FullType::Pointer(_) => 8,
            FullType::Function { .. } => 8,
            FullType::Array { elem, size } => elem
                .byte_size()
                .saturating_mul(array_size_for_layout(*size)),
            FullType::Vector { elem, lanes, .. } => elem.byte_size().saturating_mul(*lanes),
            FullType::Struct(_) => 0, // need struct_defs to compute; caller should use byte_size_with
        }
    }

    /// Total byte size with struct definitions
    pub fn byte_size_with(&self, struct_defs: &IndexMap<String, StructDef>) -> usize {
        match self {
            FullType::Struct(tag) => struct_defs.get(tag).map(|d| d.size).unwrap_or(0),
            FullType::Array { elem, size } => elem
                .byte_size_with(struct_defs)
                .saturating_mul(array_size_for_layout(*size)),
            FullType::Vector { elem, lanes, .. } => {
                elem.byte_size_with(struct_defs).saturating_mul(*lanes)
            }
            _ => self.byte_size(),
        }
    }

    /// Checked variant for declaration and layout validation paths that must
    /// reject impossible object sizes instead of allowing arithmetic to wrap.
    pub fn checked_byte_size_with(
        &self,
        struct_defs: &IndexMap<String, StructDef>,
    ) -> Option<usize> {
        match self {
            // Preserve `byte_size_with` semantics for forward declarations;
            // incomplete structs have no concrete object size yet.
            FullType::Struct(tag) => Some(struct_defs.get(tag).map_or(0, |def| def.size)),
            FullType::Array { elem, size } => elem
                .checked_byte_size_with(struct_defs)?
                .checked_mul(array_size_for_layout(*size)),
            FullType::Vector { elem, lanes, .. } => elem
                .checked_byte_size_with(struct_defs)?
                .checked_mul(*lanes),
            _ => Some(self.byte_size()),
        }
    }

    /// Returns whether this type contains a nonconstant array bound.
    ///
    /// The size APIs intentionally use a conservative estimate for lowering
    /// VLA storage, but that estimate must never make `sizeof` an integer
    /// constant expression.
    #[must_use]
    pub fn contains_vla_placeholder(&self) -> bool {
        match self {
            FullType::Array { elem, size } => {
                *size == VLA_STATIC_SCALE_FALLBACK || elem.contains_vla_placeholder()
            }
            FullType::Vector { elem, .. } => elem.contains_vla_placeholder(),
            FullType::Pointer(_)
            | FullType::Function { .. }
            | FullType::Scalar(_)
            | FullType::Struct(_) => false,
        }
    }

    /// Like [`FullType::contains_vla_placeholder`], also following complete
    /// aggregate definitions when checking a `struct` or `union` type.
    #[must_use]
    pub fn contains_vla_placeholder_with(&self, struct_defs: &IndexMap<String, StructDef>) -> bool {
        match self {
            FullType::Struct(tag) => struct_defs.get(tag).is_some_and(|def| {
                def.members.iter().any(|member| {
                    member
                        .member_full_type
                        .contains_vla_placeholder_with(struct_defs)
                })
            }),
            FullType::Array { elem, size } => {
                *size == VLA_STATIC_SCALE_FALLBACK
                    || elem.contains_vla_placeholder_with(struct_defs)
            }
            FullType::Vector { elem, .. } => elem.contains_vla_placeholder_with(struct_defs),
            FullType::Pointer(_) | FullType::Function { .. } | FullType::Scalar(_) => false,
        }
    }

    /// Alignment requirement
    pub fn alignment(&self) -> usize {
        match self {
            FullType::Scalar(t) => std::cmp::max(t.size() as usize, 1),
            FullType::Pointer(_) => 8,
            FullType::Function { .. } => 8,
            FullType::Array { elem, .. } => {
                let ea = elem.alignment();
                if self.byte_size() >= 16 {
                    std::cmp::max(ea, 16)
                } else {
                    ea
                }
            }
            FullType::Vector { elem, .. } => elem.alignment(),
            FullType::Struct(_) => 1, // need struct_defs; caller should use alignment_with
        }
    }

    /// Alignment requirement with struct definitions available.
    pub fn alignment_with(&self, struct_defs: &IndexMap<String, StructDef>) -> usize {
        match self {
            FullType::Struct(tag) => struct_defs.get(tag).map(|d| d.alignment).unwrap_or(1),
            FullType::Array { elem, .. } => elem.alignment_with(struct_defs),
            FullType::Vector { elem, .. } => elem.alignment_with(struct_defs),
            _ => self.alignment(),
        }
    }

    /// Get the element type (for arrays: inner element; for pointers: pointee)
    pub fn elem_type(&self) -> Option<&FullType> {
        match self {
            FullType::Array { elem, .. } => Some(elem),
            FullType::Pointer(inner) => Some(inner),
            FullType::Function { return_type, .. } => Some(return_type),
            FullType::Vector { elem, .. } => Some(elem),
            _ => None,
        }
    }

    /// Array-to-pointer decay
    pub fn decay(&self) -> FullType {
        match self {
            FullType::Array { elem, .. } => FullType::Pointer(elem.clone()),
            other => other.clone(),
        }
    }

    pub fn is_array(&self) -> bool {
        matches!(self, FullType::Array { .. })
    }

    pub fn is_vector(&self) -> bool {
        matches!(self, FullType::Vector { .. })
    }

    pub fn is_complex(&self) -> bool {
        matches!(self, FullType::Vector { complex: true, .. })
    }

    pub fn is_pointer(&self) -> bool {
        matches!(self, FullType::Pointer(_))
    }

    pub fn is_scalar(&self) -> bool {
        matches!(self, FullType::Scalar(_))
    }

    pub fn is_struct(&self) -> bool {
        matches!(self, FullType::Struct(_))
    }

    /// Construct from parser output (base type + ptr_info + array_dims)
    pub fn from_decl(
        base: CType,
        ptr_info: Option<(CType, usize)>,
        array_dims: &Option<Vec<usize>>,
    ) -> FullType {
        let base_full = if let Some((base_t, depth)) = ptr_info {
            let mut t = FullType::Scalar(base_t);
            for _ in 0..depth {
                t = FullType::Pointer(Box::new(t));
            }
            t
        } else {
            FullType::Scalar(base)
        };

        if let Some(dims) = array_dims {
            // Build array type from innermost to outermost
            let mut t = if ptr_info.is_some() {
                base_full
            } else {
                FullType::Scalar(base)
            };
            for &dim in dims.iter().rev() {
                if dim > 0 {
                    t = FullType::Array {
                        elem: Box::new(t),
                        size: dim,
                    };
                }
            }
            t
        } else {
            base_full
        }
    }
}

/// Struct/Union definition: member layout information
#[derive(Debug, Clone)]
pub struct StructDef {
    pub tag: String,
    pub members: Vec<StructMember>,
    pub size: usize,
    pub alignment: usize,
    pub is_union: bool,
}

/// System V ABI classification for struct parameter passing
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamClass {
    Integer,
    Sse,
    Memory,
}

impl StructDef {
    fn type_contains_unaligned_fields(
        full_type: &FullType,
        struct_defs: &IndexMap<String, StructDef>,
    ) -> bool {
        match full_type {
            FullType::Struct(tag) => struct_defs
                .get(tag)
                .is_some_and(|def| def.has_unaligned_fields(struct_defs)),
            FullType::Array { elem, .. } | FullType::Vector { elem, .. } => {
                Self::type_contains_unaligned_fields(elem, struct_defs)
            }
            FullType::Scalar(_) | FullType::Pointer(_) | FullType::Function { .. } => false,
        }
    }

    fn has_unaligned_fields(&self, struct_defs: &IndexMap<String, StructDef>) -> bool {
        self.members.iter().any(|mem| {
            let alignment = mem.member_full_type.alignment_with(struct_defs).max(1);
            mem.offset % alignment != 0
                || Self::type_contains_unaligned_fields(&mem.member_full_type, struct_defs)
        })
    }

    /// Classify a struct for System V ABI parameter/return passing.
    /// Returns a list of ParamClass for each 8-byte chunk, or Memory if passed on stack.
    /// Flatten all fields to (byte_offset, scalar_type) pairs,
    /// recursing into nested structs and arrays.
    fn flatten_fields(
        &self,
        base_offset: usize,
        struct_defs: &IndexMap<String, StructDef>,
    ) -> Vec<(usize, CType)> {
        let mut fields = Vec::new();
        for mem in &self.members {
            fields.extend(self.flatten_member_fields(mem, base_offset, struct_defs));
        }
        fields
    }

    fn flatten_member_fields(
        &self,
        mem: &StructMember,
        base_offset: usize,
        struct_defs: &IndexMap<String, StructDef>,
    ) -> Vec<(usize, CType)> {
        let Some(abs_offset) = base_offset.checked_add(mem.offset) else {
            return vec![];
        };
        match &mem.member_full_type {
            FullType::Struct(tag) => {
                if let Some(def) = struct_defs.get(tag) {
                    def.flatten_fields(abs_offset, struct_defs)
                } else {
                    vec![]
                }
            }
            FullType::Array { elem, .. } => {
                let mut inner = elem.as_ref();
                while let FullType::Array { elem: e, .. } = inner {
                    inner = e;
                }
                if let FullType::Struct(tag) = inner {
                    if let Some(def) = struct_defs.get(tag) {
                        let stride = def.size.max(1);
                        let total_elems = mem.size / stride;
                        return (0..total_elems)
                            .filter_map(|i| {
                                i.checked_mul(def.size)
                                    .and_then(|delta| abs_offset.checked_add(delta))
                            })
                            .flat_map(|offset| def.flatten_fields(offset, struct_defs))
                            .collect();
                    }
                    return vec![];
                }
                let scalar_type = inner.to_ctype();
                let elem_size = inner.byte_size_with(struct_defs);
                if elem_size == 0 {
                    return vec![];
                }
                let total_elems = mem.size / elem_size;
                (0..total_elems)
                    .filter_map(|i| {
                        i.checked_mul(elem_size)
                            .and_then(|delta| abs_offset.checked_add(delta))
                            .map(|offset| (offset, scalar_type))
                    })
                    .collect()
            }
            FullType::Vector { elem, lanes, .. } => {
                let elem_size = elem.byte_size_with(struct_defs);
                if elem_size == 0 {
                    return vec![];
                }
                let scalar_type = elem.to_ctype();
                (0..*lanes)
                    .filter_map(|i| {
                        i.checked_mul(elem_size)
                            .and_then(|delta| abs_offset.checked_add(delta))
                            .map(|offset| (offset, scalar_type))
                    })
                    .collect()
            }
            _ => vec![(abs_offset, mem.member_type)],
        }
    }

    pub fn classify_with(&self, struct_defs: &IndexMap<String, StructDef>) -> Vec<ParamClass> {
        // SysV requires aggregates containing unaligned fields to be passed
        // in memory, even when their total size fits in two eightbytes.
        if self.size > 16 || self.has_unaligned_fields(struct_defs) {
            return vec![ParamClass::Memory];
        }
        let num_eightbytes = self.size.div_ceil(8);

        if self.is_union {
            // Union classification: check ALL members
            // Start with NO_CLASS, then merge each member's classification
            // SSE + SSE = SSE, SSE + INTEGER = INTEGER, INTEGER + INTEGER = INTEGER
            let mut classes = vec![None::<ParamClass>; num_eightbytes];
            for mem in &self.members {
                // Get the classification this member would produce
                let mem_classes = match &mem.member_full_type {
                    FullType::Struct(tag) => {
                        if let Some(def) = struct_defs.get(tag) {
                            def.classify_with(struct_defs)
                        } else {
                            vec![ParamClass::Integer]
                        }
                    }
                    _ => {
                        // Scalar/array/pointer member: classify only covered eightbytes
                        let fields = self.flatten_member_fields(mem, 0, struct_defs);
                        let mem_ebs = mem.size.div_ceil(8);
                        let mut mc = Vec::new();
                        for eb in 0..std::cmp::min(mem_ebs, num_eightbytes) {
                            let mut class = None;
                            for (off, ctype) in &fields {
                                if off / 8 == eb {
                                    merge_param_class(&mut class, abi_field_class(*ctype));
                                }
                            }
                            mc.push(class.unwrap_or(ParamClass::Integer));
                        }
                        mc
                    }
                };
                // Merge: INTEGER dominates SSE
                for (eb, mc) in mem_classes.iter().enumerate() {
                    if eb >= num_eightbytes {
                        break;
                    }
                    match (&classes[eb], mc) {
                        (None, _) => classes[eb] = Some(*mc),
                        (Some(ParamClass::Integer), _) => {} // INTEGER stays
                        (Some(ParamClass::Sse), ParamClass::Integer) => {
                            classes[eb] = Some(ParamClass::Integer)
                        }
                        (Some(ParamClass::Sse), ParamClass::Sse) => {} // SSE stays
                        _ => {}
                    }
                }
            }
            classes
                .iter()
                .map(|c| c.unwrap_or(ParamClass::Integer))
                .collect()
        } else {
            // Struct classification: based on flattened fields
            let mut classes = vec![None::<ParamClass>; num_eightbytes];
            let fields = self.flatten_fields(0, struct_defs);
            for (offset, ctype) in &fields {
                let eb = offset / 8;
                if eb < num_eightbytes {
                    merge_param_class(&mut classes[eb], abi_field_class(*ctype));
                }
            }
            classes
                .into_iter()
                .map(|c| c.unwrap_or(ParamClass::Integer))
                .collect()
        }
    }

    pub fn classify(&self) -> Vec<ParamClass> {
        // Legacy version without struct_defs — works for structs without nested structs
        self.classify_with(&IndexMap::new())
    }
}

fn abi_field_class(ctype: CType) -> ParamClass {
    if matches!(ctype, CType::Float | CType::Double) {
        ParamClass::Sse
    } else {
        ParamClass::Integer
    }
}

fn merge_param_class(current: &mut Option<ParamClass>, incoming: ParamClass) {
    match current {
        None => *current = Some(incoming),
        Some(ParamClass::Sse) if incoming == ParamClass::Integer => {
            *current = Some(ParamClass::Integer)
        }
        _ => {}
    }
}

#[derive(Debug, Clone)]
pub struct StructMember {
    pub name: String,
    pub member_type: CType,
    pub member_full_type: FullType,
    pub flexible_array: bool,
    pub offset: usize,
    pub size: usize,
    pub bit_width: Option<u8>,
    pub bit_offset: u8,
    pub reverse_storage_order: bool,
}

impl StructDef {
    pub fn from_declaration(
        declaration: &StructDeclaration,
        struct_defs: &IndexMap<String, StructDef>,
    ) -> Result<Self, String> {
        Self::from_members_ex(
            &declaration.tag,
            &declaration.members,
            struct_defs,
            declaration.is_union,
            declaration.packed,
            declaration.alignment,
            declaration.reverse_storage_order,
        )
    }

    /// Compute layout from member declarations
    pub fn from_members(
        tag: &str,
        members: &[MemberDeclaration],
        struct_defs: &IndexMap<String, StructDef>,
    ) -> Result<Self, String> {
        Self::from_members_ex(tag, members, struct_defs, false, false, None, false)
    }

    pub fn from_members_union(
        tag: &str,
        members: &[MemberDeclaration],
        struct_defs: &IndexMap<String, StructDef>,
    ) -> Result<Self, String> {
        Self::from_members_ex(tag, members, struct_defs, true, false, None, false)
    }

    fn from_members_ex(
        tag: &str,
        members: &[MemberDeclaration],
        struct_defs: &IndexMap<String, StructDef>,
        is_union: bool,
        packed: bool,
        aggregate_alignment: Option<std::num::NonZeroUsize>,
        reverse_storage_order: bool,
    ) -> Result<Self, String> {
        let mut offset = 0usize;
        let mut max_align = 1usize;
        let mut max_size = 0usize;
        let mut laid_out = Vec::new();
        let mut bit_unit_offset = 0usize;
        let mut bit_unit_size = 0usize;
        let mut bit_unit_align = 1usize;
        let mut next_bit_offset = 0usize;

        for m in members {
            let (m_size, natural_align) = member_size_align(&m.member_full_type, struct_defs)?;
            let member_packed = packed || m.packed;
            let layout_align = if member_packed { 1 } else { natural_align };
            let m_align = m
                .alignment
                .map_or(layout_align, |a| a.get().max(layout_align));
            if let Some(width) = m.bit_width {
                if !matches!(
                    m.member_full_type,
                    FullType::Scalar(
                        CType::Char
                            | CType::SChar
                            | CType::UChar
                            | CType::Bool
                            | CType::Short
                            | CType::UShort
                            | CType::Int
                            | CType::UInt
                            | CType::Long
                            | CType::ULong
                            | CType::Int128
                            | CType::UInt128
                    )
                ) {
                    return Err(format!("bit-field '{}' must have integer type", m.name));
                }
                let storage_bits = m_size
                    .checked_mul(8)
                    .ok_or_else(|| format!("bit-field '{}' storage size is too large", m.name))?;
                if width as usize > storage_bits {
                    return Err(format!(
                        "bit-field '{}' width {} exceeds storage width {}",
                        m.name, width, storage_bits
                    ));
                }
                let (storage_size, storage_align, storage_type) = if member_packed {
                    (m_size, 1, m.member_type)
                } else if m_size > 4 && width <= 32 {
                    (
                        4,
                        4,
                        if m.member_type.is_signed() {
                            CType::Int
                        } else {
                            CType::UInt
                        },
                    )
                } else {
                    (m_size, m_align, m.member_type)
                };
                let zero_width_align = if member_packed && width == 0 {
                    natural_align
                } else {
                    storage_align
                };
                let storage_bits = storage_size
                    .checked_mul(8)
                    .ok_or_else(|| format!("bit-field '{}' storage size is too large", m.name))?;
                if width == 0 {
                    if !m.name.is_empty() {
                        return Err("zero-width bit-field may not have a name".to_string());
                    }
                    if !is_union {
                        offset = round_up_to(offset, zero_width_align)?;
                    }
                    next_bit_offset = 0;
                    bit_unit_size = 0;
                    bit_unit_align = zero_width_align;
                    max_align = max_align.max(m_align);
                    continue;
                }

                if is_union {
                    if !m.name.is_empty() {
                        laid_out.push(StructMember {
                            name: m.name.clone(),
                            member_type: storage_type,
                            member_full_type: m.member_full_type.clone(),
                            flexible_array: m.flexible_array,
                            offset: 0,
                            size: storage_size,
                            bit_width: Some(width),
                            bit_offset: 0,
                            reverse_storage_order,
                        });
                    }
                    let occupied_size = if member_packed {
                        usize::from(width).div_ceil(8).max(1)
                    } else {
                        storage_size
                    };
                    max_size = max_size.max(occupied_size);
                    max_align = max_align.max(storage_align);
                    continue;
                }

                let current_bit_end = next_bit_offset
                    .checked_add(width as usize)
                    .ok_or_else(|| format!("bit-field '{}' offset is too large", m.name))?;
                let needs_new_unit = bit_unit_size == 0
                    || bit_unit_size != storage_size
                    || bit_unit_align != storage_align
                    || current_bit_end > storage_bits;
                if needs_new_unit {
                    offset = round_up_to(offset, storage_align)?;
                    bit_unit_offset = offset;
                    bit_unit_size = storage_size;
                    bit_unit_align = storage_align;
                    next_bit_offset = 0;
                    offset = offset
                        .checked_add(storage_size)
                        .ok_or_else(|| format!("struct '{}' layout is too large", tag))?;
                }
                if !m.name.is_empty() {
                    laid_out.push(StructMember {
                        name: m.name.clone(),
                        member_type: storage_type,
                        member_full_type: m.member_full_type.clone(),
                        flexible_array: m.flexible_array,
                        offset: bit_unit_offset,
                        size: storage_size,
                        bit_width: Some(width),
                        bit_offset: if reverse_storage_order {
                            (storage_bits - next_bit_offset - width as usize) as u8
                        } else {
                            next_bit_offset as u8
                        },
                        reverse_storage_order,
                    });
                }
                next_bit_offset = if needs_new_unit {
                    width as usize
                } else {
                    current_bit_end
                };
                max_align = max_align.max(storage_align);
                continue;
            }

            if next_bit_offset > 0 {
                offset = bit_unit_offset
                    .checked_add(next_bit_offset.div_ceil(8))
                    .ok_or_else(|| format!("struct '{}' layout is too large", tag))?;
            }
            next_bit_offset = 0;
            bit_unit_size = 0;
            if is_union {
                // Union: all members at offset 0
                if m.name.is_empty() {
                    if let FullType::Struct(nested_tag) = &m.member_full_type {
                        if let Some(nested) = struct_defs.get(nested_tag) {
                            for nested_member in &nested.members {
                                laid_out.push(nested_member.clone());
                            }
                        }
                    }
                } else {
                    laid_out.push(StructMember {
                        name: m.name.clone(),
                        member_type: m.member_type,
                        member_full_type: m.member_full_type.clone(),
                        flexible_array: m.flexible_array,
                        offset: 0,
                        size: m_size,
                        bit_width: None,
                        bit_offset: 0,
                        reverse_storage_order: false,
                    });
                }
                if m_size > max_size {
                    max_size = m_size;
                }
            } else {
                // Struct: sequential layout with alignment
                offset = round_up_to(offset, m_align)?;
                if m.name.is_empty() {
                    if let FullType::Struct(nested_tag) = &m.member_full_type {
                        if let Some(nested) = struct_defs.get(nested_tag) {
                            for nested_member in &nested.members {
                                let mut flattened = nested_member.clone();
                                flattened.offset =
                                    flattened.offset.checked_add(offset).ok_or_else(|| {
                                        format!("struct '{}' layout is too large", tag)
                                    })?;
                                laid_out.push(flattened);
                            }
                        }
                    }
                } else {
                    laid_out.push(StructMember {
                        name: m.name.clone(),
                        member_type: m.member_type,
                        member_full_type: m.member_full_type.clone(),
                        flexible_array: m.flexible_array,
                        offset,
                        size: m_size,
                        bit_width: None,
                        bit_offset: 0,
                        reverse_storage_order: false,
                    });
                }
                offset = offset
                    .checked_add(m_size)
                    .ok_or_else(|| format!("struct '{}' layout is too large", tag))?;
            }
            if m_align > max_align {
                max_align = m_align;
            }
        }

        if let Some(alignment) = aggregate_alignment {
            max_align = max_align.max(alignment.get());
        }

        let total_size = if is_union {
            // Union size = max member size, padded to alignment
            round_up_to(max_size, max_align)?
        } else {
            round_up_to(offset, max_align)?
        };

        Ok(StructDef {
            tag: tag.to_string(),
            members: laid_out,
            size: total_size,
            alignment: max_align,
            is_union,
        })
    }

    #[must_use]
    pub fn find_member(&self, name: &str) -> Option<&StructMember> {
        self.members.iter().find(|m| m.name == name)
    }
}

fn round_up_to(value: usize, alignment: usize) -> Result<usize, String> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(format!("invalid alignment {}", alignment));
    }
    let mask = alignment - 1;
    value
        .checked_add(mask)
        .map(|rounded| rounded & !mask)
        .ok_or_else(|| "struct layout is too large".to_string())
}

fn member_size_align(
    ft: &FullType,
    struct_defs: &IndexMap<String, StructDef>,
) -> Result<(usize, usize), String> {
    match ft {
        FullType::Scalar(t) => Ok((
            std::cmp::max(t.size() as usize, 1),
            std::cmp::max(t.size() as usize, 1),
        )),
        FullType::Pointer(_) => Ok((8, 8)),
        FullType::Function { .. } => Ok((8, 8)),
        FullType::Array { elem, size } => {
            let (elem_size, elem_align) = member_size_align(elem, struct_defs)?;
            let total = elem_size
                .checked_mul(array_size_for_layout(*size))
                .ok_or_else(|| "struct member array size is too large".to_string())?;
            // Inside structs, array alignment is just the element alignment
            Ok((total, elem_align))
        }
        FullType::Vector { elem, lanes, .. } => {
            let (elem_size, elem_align) = member_size_align(elem, struct_defs)?;
            let total = elem_size
                .checked_mul(*lanes)
                .ok_or_else(|| "struct member vector size is too large".to_string())?;
            Ok((total, elem_align))
        }
        FullType::Struct(tag) => {
            if let Some(def) = struct_defs.get(tag) {
                Ok((def.size, def.alignment))
            } else {
                Err(format!("Undefined struct: {}", tag))
            }
        }
    }
}

// ============================================================
// Static Initializer (for arrays and scalars)
// ============================================================

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone)]
pub enum StaticInit {
    IntInit(i32),
    LongInit(i64),
    Int128Init(i128),
    UIntInit(u32),
    ULongInit(u64),
    UInt128Init(u128),
    ShortInit(i16),
    UShortInit(u16),
    CharInit(i8),
    UCharInit(u8),
    DoubleInit(f64),
    FloatInit(f32),
    LongDoubleInit(f64),
    ZeroInit(usize),                      // zero-fill N bytes
    StringInit(String, bool),             // (string_content, null_terminated) → .asciz or .ascii
    PointerInit(String),                  // label name → .quad label_name
    PointerInitOffset(String, i64),       // label + addend → .quad label_name+addend
    LabelDiffInit(String, String, usize), // left - right → .long/.quad label difference
}

// ============================================================
// Tokens
// ============================================================

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    Identifier(String),
    IntLiteral(i64),
    LongLiteral(i64),
    Int128Literal(i128),
    UIntLiteral(i64),
    ULongLiteral(i64),
    UInt128Literal(u128),
    DoubleLiteral(f64),
    LongDoubleLiteral(f64),
    ImaginaryIntLiteral(i64),
    ImaginaryDoubleLiteral(f64),
    CharLiteral(i64),
    StringLiteral(String),
    WideStringLiteral(String),
    Utf16StringLiteral(String),
    Utf32StringLiteral(String),
    // Keywords
    KWChar,
    KWSizeOf,
    KWAlignOf,
    KWAlignAs,
    KWGeneric,
    KWAutoType,
    KWStruct,
    KWUnion,
    KWInt,
    KWLong,
    KWUnsigned,
    KWSigned,
    KWDouble,
    KWFloat,
    KWVoid,
    KWReturn,
    KWIf,
    KWElse,
    KWWhile,
    KWFor,
    KWDo,
    KWBreak,
    KWContinue,
    KWGoto,
    KWSwitch,
    KWCase,
    KWDefault,
    KWStatic,
    KWExtern,
    KWTypedef,
    KWEnum,
    KWConst,
    KWVolatile,
    KWInline,
    KWAtomic,
    KWThreadLocal,
    KWStaticAssert,
    KWRegister,
    KWAuto,
    KWBool,
    KWRestrict,
    KWShort,
    KWNoreturn,
    KWTypeOf,
    KWTypeOfUnqual,
    AttributeAligned(String),
    AttributeAlignedNoreturn(String),
    AttributePacked,
    AttributePackedAligned(String),
    AttributePackedAlignedNoreturn(String),
    AttributeTransparentUnion,
    AttributeNoreturn,
    AttributeNoInstrumentFunction,
    AttributeDeprecated(Option<String>),
    AttributeAlias(String),
    AttributeMode(String),
    AttributeVectorSize(String),
    AttributeScalarStorageOrderReverse,
    Skip,
    Ellipsis, // ...

    // Punctuation
    OpenParen,
    CloseParen,
    OpenBrace,
    CloseBrace,
    Semicolon,
    Comma,
    OpenBracket,
    CloseBracket,
    // Unary / Binary operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Tilde,
    Bang,
    Ampersand,
    Pipe,
    Caret,
    ShiftLeft,
    ShiftRight,
    LogicalAnd,
    LogicalOr,
    EqualEqual,
    NotEqual,
    LessThan,
    GreaterThan,
    LessEqual,
    GreaterEqual,
    // Assignment
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    PercentAssign,
    AmpersandAssign,
    PipeAssign,
    CaretAssign,
    ShiftLeftAssign,
    ShiftRightAssign,
    // Increment / Decrement
    Increment,
    Decrement,
    // Ternary
    Question,
    Colon,
    // Struct member access
    Dot,
    Arrow,
}

// ============================================================
// AST
// ============================================================

#[derive(Debug, Clone)]
pub enum UnaryOp {
    Negate,
    Complement,
    LogicalNot,
    PreIncrement,
    PreDecrement,
    PostIncrement,
    PostDecrement,
    AddrOf,
    Deref,
    RealPart,
    ImagPart,
}

#[derive(Debug, Clone)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitwiseAnd,
    BitwiseNand,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
    LogicalAnd,
    LogicalOr,
    Equal,
    NotEqual,
    LessThan,
    GreaterThan,
    LessEqual,
    GreaterEqual,
}

#[derive(Debug, Clone)]
pub enum Designator {
    Field(String),
    Index(Box<Exp>),
    IndexRange(Box<Exp>, Box<Exp>),
}

#[derive(Debug, Clone)]
pub enum Exp {
    Constant(i64),
    LongConstant(i64),
    Int128Constant(i128),
    UIntConstant(i64),
    ULongConstant(i64),
    UInt128Constant(u128),
    DoubleConstant(f64),
    LongDoubleConstant(f64),
    ImaginaryIntConstant(i64),
    ImaginaryDoubleConstant(f64),
    StringLiteral(String),
    WideStringLiteral(String),
    Utf16StringLiteral(String),
    Utf32StringLiteral(String),
    Var(String),
    LabelAddress(String),
    Cast(CType, Option<FullType>, Box<Exp>),
    Unary(UnaryOp, Box<Exp>),
    Binary(BinaryOp, Box<Exp>, Box<Exp>),
    Assign(Box<Exp>, Box<Exp>),
    CompoundAssign(BinaryOp, Box<Exp>, Box<Exp>),
    Conditional(Box<Exp>, Box<Exp>, Box<Exp>),
    BuiltinExpect(Box<Exp>, Vec<Exp>),
    FunctionCall(String, Vec<Exp>),
    ImplicitFunctionCall(String, Vec<Exp>),
    Subscript(Box<Exp>, Box<Exp>), // arr[index]
    ArrayInit(Vec<Exp>),           // {1, 2, 3} or {{1,2}, {3,4}}
    DesignatedInit(Vec<Designator>, Box<Exp>),
    SizeOf(Box<Exp>),                                         // sizeof expr
    SizeOfType(CType, FullType),                              // sizeof(type)
    AlignOfType(FullType),                                    // _Alignof(type)
    Dot(Box<Exp>, String),                                    // expr.member
    Arrow(Box<Exp>, String),                                  // expr->member
    Comma(Box<Exp>, Box<Exp>),                                // a, b — evaluate both, result is b
    StatementExpr(Block, Option<Box<Exp>>, Option<FullType>), // GNU ({ ... expr; })
    IndirectCall(Box<Exp>, Vec<Exp>), // expr(args) — call through function pointer expression
    Unreachable,
    AtomicFence,
    AtomicFetch {
        op: BinaryOp,
        ptr: Box<Exp>,
        arg: Box<Exp>,
        return_old: bool,
    },
    AtomicExchange {
        ptr: Box<Exp>,
        value: Box<Exp>,
    },
    AtomicCompareExchange {
        ptr: Box<Exp>,
        expected: Box<Exp>,
        desired: Box<Exp>,
    },
    AtomicCompareSwap {
        ptr: Box<Exp>,
        expected: Box<Exp>,
        desired: Box<Exp>,
        return_old: bool,
    },
}

#[derive(Debug, Clone)]
pub enum ForInit {
    Declaration(VarDeclaration),
    Expression(Option<Exp>),
}

#[derive(Debug, Clone)]
pub enum Statement {
    Return(Option<Exp>),
    Expression(Exp),
    If(Exp, Box<Statement>, Option<Box<Statement>>),
    Block(Block),
    While {
        condition: Exp,
        body: Box<Statement>,
        label: String,
    },
    DoWhile {
        body: Box<Statement>,
        condition: Exp,
        label: String,
    },
    For {
        init: Box<ForInit>,
        condition: Option<Exp>,
        post: Option<Exp>,
        body: Box<Statement>,
        label: String,
    },
    Break(String),
    Continue(String),
    Goto(String),
    IndirectGoto(Exp),
    Label(String, Box<Statement>),
    Switch {
        control: Exp,
        body: Box<Statement>,
        label: String,
        cases: Vec<SwitchCase>,
    },
    Case {
        value: Exp,
        end_value: Option<Exp>,
        body: Box<Statement>,
        label: String,
    },
    Default {
        body: Box<Statement>,
        label: String,
    },
    Null,
}

#[derive(Debug, Clone)]
pub struct SwitchCase {
    pub value: Option<SwitchCaseValue>, // None = default
    pub end_value: Option<SwitchCaseValue>,
    pub label: String,
}

#[derive(Debug, Clone, Copy)]
pub struct SwitchCaseValue {
    pub value: i128,
    pub ctype: CType,
}

pub type Block = Vec<BlockItem>;

#[derive(Debug, Clone)]
pub enum BlockItem {
    Declaration(Declaration),
    Statement(Statement),
}

#[derive(Debug, Clone, PartialEq)]
pub enum StorageClass {
    Static,
    Extern,
    Typedef,
    ThreadLocal,
    StaticThreadLocal,
    ExternThreadLocal,
}

impl StorageClass {
    #[must_use]
    pub fn is_static(&self) -> bool {
        matches!(self, Self::Static | Self::StaticThreadLocal)
    }

    #[must_use]
    pub fn is_extern(&self) -> bool {
        matches!(self, Self::Extern | Self::ExternThreadLocal)
    }

    #[must_use]
    pub fn is_typedef(&self) -> bool {
        matches!(self, Self::Typedef)
    }

    #[must_use]
    pub fn is_thread_local(&self) -> bool {
        matches!(
            self,
            Self::ThreadLocal | Self::StaticThreadLocal | Self::ExternThreadLocal
        )
    }

    #[must_use]
    pub fn with_static(self) -> Self {
        match self {
            Self::ThreadLocal => Self::StaticThreadLocal,
            other => other,
        }
    }

    #[must_use]
    pub fn with_extern(self) -> Self {
        match self {
            Self::ThreadLocal => Self::ExternThreadLocal,
            other => other,
        }
    }

    #[must_use]
    pub fn with_thread_local(self) -> Self {
        match self {
            Self::Static => Self::StaticThreadLocal,
            Self::Extern => Self::ExternThreadLocal,
            other => other,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VarDeclaration {
    pub name: String,
    pub var_type: CType,
    /// For pointer variables: (base_type, pointer_depth)
    pub ptr_info: Option<(CType, usize)>,
    /// For array variables: (element_type, dimensions) e.g., int a[2][3] → (Int, [2,3])
    pub array_dims: Option<Vec<usize>>,
    /// Full derived type from declarator (includes pointer-to-array info)
    pub decl_full_type: Option<FullType>,
    /// Runtime byte size for VLA-derived objects, or pointer element size for
    /// pointers to VLA-derived aggregate types.
    pub dynamic_size: Option<Box<Exp>>,
    pub init: Option<Exp>,
    pub storage_class: Option<StorageClass>,
    pub alignment: Option<std::num::NonZeroUsize>,
    pub alias: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FunctionDeclaration {
    pub name: String,
    pub return_type: CType,
    pub return_ptr_info: Option<PtrInfo>,
    pub return_full_type: Option<FullType>,
    /// Params: (name, type, optional ptr_info)
    pub params: Vec<ParamDecl>,
    /// Full types for each parameter (for proper multi-dim array tracking)
    pub param_full_types: Vec<FullType>,
    /// Runtime VLA bounds captured while parsing parameter declarators.
    pub param_vla_bounds: Vec<Exp>,
    /// Parameters annotated with deprecated and their optional message.
    pub deprecated_params: Vec<DeprecatedParam>,
    /// True if the prototype ends in `...`.
    pub variadic: bool,
    /// True for GNU/C23-style declarations with no fixed parameters: `f(...)`.
    pub zero_fixed_variadic: bool,
    /// True if the declaration came from an old-style identifier-list function.
    pub old_style: bool,
    /// True if the declaration used a noreturn spelling.
    pub noreturn: bool,
    /// True if function instrumentation hooks must not be emitted.
    pub no_instrument_function: bool,
    /// True if the declaration used an inline spelling.
    pub is_inline: bool,
    pub body: Option<Block>,
    pub storage_class: Option<StorageClass>,
}

#[derive(Debug, Clone)]
pub struct MemberDeclaration {
    pub name: String,
    pub member_type: CType,
    pub member_full_type: FullType,
    pub bit_width: Option<u8>,
    pub flexible_array: bool,
    pub alignment: Option<std::num::NonZeroUsize>,
    pub packed: bool,
}

#[derive(Debug, Clone)]
pub struct StructDeclaration {
    pub tag: String,
    pub members: Vec<MemberDeclaration>, // empty = incomplete type
    pub is_union: bool,
    pub transparent_union: bool,
    pub packed: bool,
    pub alignment: Option<std::num::NonZeroUsize>,
    pub reverse_storage_order: bool,
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone)]
pub enum Declaration {
    FunDecl(FunctionDeclaration),
    VarDecl(VarDeclaration),
    StructDecl(StructDeclaration),
    TypedefDecl, // No-op: fully resolved at parse time
}

#[derive(Debug)]
pub struct Program {
    pub declarations: Vec<Declaration>,
}

// ============================================================
// TACKY IR (Three-Address Code)
// ============================================================

#[derive(Debug, Clone)]
pub enum TackyVal {
    Constant(i64),
    Int128Constant(i128),
    UInt128Constant(u128),
    DoubleConstant(f64),
    Var(String),
}

impl PartialEq for TackyVal {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Constant(a), Self::Constant(b)) => a == b,
            (Self::Int128Constant(a), Self::Int128Constant(b)) => a == b,
            (Self::UInt128Constant(a), Self::UInt128Constant(b)) => a == b,
            (Self::DoubleConstant(a), Self::DoubleConstant(b)) => a.to_bits() == b.to_bits(),
            (Self::Var(a), Self::Var(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TackyUnaryOp {
    Negate,
    Complement,
    LogicalNot,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TackyBinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitwiseAnd,
    BitwiseNand,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
    Equal,
    NotEqual,
    LessThan,
    LessEqual,
    GreaterThan,
    GreaterEqual,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TackyInstr {
    Nop,
    Return(TackyVal),
    Unary {
        op: TackyUnaryOp,
        src: TackyVal,
        dst: TackyVal,
    },
    Binary {
        op: TackyBinaryOp,
        left: TackyVal,
        right: TackyVal,
        dst: TackyVal,
    },
    Copy {
        src: TackyVal,
        dst: TackyVal,
    },
    Jump(String),
    NonlocalJump(String),
    JumpIndirect(TackyVal),
    JumpIfZero(TackyVal, String),
    JumpIfNotZero(TackyVal, String),
    Label(String),
    LoadLabelAddress(String, TackyVal),
    FrameAddress {
        dst: TackyVal,
    },
    BuiltinSetjmp {
        buf: TackyVal,
        dst: TackyVal,
        label: String,
        end_label: String,
    },
    BuiltinLongjmp {
        buf: TackyVal,
        value: TackyVal,
    },
    Unreachable,
    FunCall {
        name: String,
        args: Vec<TackyVal>,
        dst: TackyVal,
        /// Indices of args that must be passed on the stack (MEMORY-class struct eightbytes)
        stack_arg_indices: std::collections::HashSet<usize>,
        /// Stack-passed aggregate blocks: (flattened arg index containing source address, byte size, alignment).
        memory_arg_blocks: Vec<(usize, usize, usize)>,
        /// Groups of consecutive args that form struct eightbytes (start_idx, count, is_sse_vec)
        struct_arg_groups: Vec<(usize, usize, Vec<bool>)>,
        /// True if the direct call target has a `...` prototype.
        variadic: bool,
        /// Number of flattened TACKY arguments belonging to fixed prototype parameters.
        fixed_flat_arg_count: usize,
        /// True when arg 0 is a caller-provided return buffer.
        hidden_return: bool,
        /// True if calling through a function pointer variable
        indirect: bool,
    },
    VaStart {
        dst: TackyVal,
    },
    SignExtend {
        src: TackyVal,
        dst: TackyVal,
    },
    ZeroExtend {
        src: TackyVal,
        dst: TackyVal,
    },
    Truncate {
        src: TackyVal,
        dst: TackyVal,
    },
    IntToDouble {
        src: TackyVal,
        dst: TackyVal,
    },
    IntToFloat {
        src: TackyVal,
        dst: TackyVal,
    },
    DoubleToInt {
        src: TackyVal,
        dst: TackyVal,
    },
    FloatToInt {
        src: TackyVal,
        dst: TackyVal,
    },
    UIntToDouble {
        src: TackyVal,
        dst: TackyVal,
    },
    UIntToFloat {
        src: TackyVal,
        dst: TackyVal,
    },
    DoubleToUInt {
        src: TackyVal,
        dst: TackyVal,
    },
    FloatToUInt {
        src: TackyVal,
        dst: TackyVal,
    },
    FloatToDouble {
        src: TackyVal,
        dst: TackyVal,
    },
    DoubleToFloat {
        src: TackyVal,
        dst: TackyVal,
    },
    AtomicFence,
    AtomicFetch {
        op: TackyBinaryOp,
        ptr: TackyVal,
        arg: TackyVal,
        return_old: bool,
        dst: TackyVal,
    },
    AtomicExchange {
        ptr: TackyVal,
        value: TackyVal,
        dst: TackyVal,
    },
    AtomicCompareExchange {
        ptr: TackyVal,
        expected: TackyVal,
        desired: TackyVal,
        dst: TackyVal,
    },
    AtomicCompareSwap {
        ptr: TackyVal,
        expected: TackyVal,
        desired: TackyVal,
        return_old: bool,
        dst: TackyVal,
    },
    GetAddress {
        src: TackyVal,
        dst: TackyVal,
    },
    Load {
        src_ptr: TackyVal,
        dst: TackyVal,
    },
    Store {
        src: TackyVal,
        dst_ptr: TackyVal,
    },
    /// Copy src value to dst_name at byte offset. dst_name is an aggregate (array/struct).
    CopyToOffset {
        src: TackyVal,
        dst_name: String,
        offset: i64,
    },
    /// Read from aggregate at byte offset: dst = src_name[offset]
    CopyFromOffset {
        src_name: String,
        offset: i64,
        dst: TackyVal,
    },
    /// Add pointer + index * scale → dst
    AddPtr {
        ptr: TackyVal,
        index: TackyVal,
        scale: i64,
        dst: TackyVal,
    },
    /// Whole-struct copy annotation (no-op in codegen, used by copy propagation)
    CopyStruct {
        src_name: String,
        dst_name: String,
    },
}

#[derive(Debug, Clone)]
pub struct TackyFunction {
    pub name: String,
    pub return_type: CType,
    pub params: Vec<String>,
    pub global: bool,
    pub body: Vec<TackyInstr>,
    /// Params that must be passed on the stack (MEMORY-class struct eightbytes)
    pub stack_params: std::collections::HashSet<String>,
    /// Stack-passed aggregate blocks: (flattened param index, original aggregate name, byte size).
    pub memory_param_blocks: Vec<(usize, String, usize)>,
    /// Groups of consecutive params that form struct eightbytes.
    /// Each (start_idx, count, is_sse_vec) means params[start..start+count]
    /// must ALL fit in registers or ALL go on the stack.
    /// is_sse_vec indicates which eightbytes need SSE vs integer registers.
    pub struct_param_groups: Vec<(usize, usize, Vec<bool>)>,
}

#[derive(Debug, Clone)]
pub struct TackyStaticVar {
    pub name: String,
    pub global: bool,
    pub thread_local: bool,
    pub alignment: usize,
    pub init_values: Vec<StaticInit>,
}

#[derive(Debug, Clone)]
pub struct TackyStaticConstant {
    pub name: String,
    pub alignment: usize,
    pub init: StaticInit,
}

#[derive(Debug, Clone)]
pub enum TackyTopLevel {
    Function(TackyFunction),
    StaticVar(TackyStaticVar),
    StaticConstant(TackyStaticConstant),
    Alias { name: String, target: String },
}

#[derive(Debug, Clone)]
pub struct TackyProgram {
    pub top_level: Vec<TackyTopLevel>,
    pub global_vars: std::collections::HashSet<String>,
    pub thread_local_vars: std::collections::HashSet<String>,
    pub symbol_types: IndexMap<String, CType>,
    pub symbol_alignments: IndexMap<String, usize>,
    /// Array/struct storage sizes
    pub array_sizes: IndexMap<String, usize>,
    /// Struct definitions for ABI classification
    pub struct_defs: IndexMap<String, StructDef>,
    /// Map from variable name to struct tag
    pub var_struct_tags: std::collections::HashMap<String, String>,
}

// ============================================================
// Assembly IR (re-exported from backend::common)
// ============================================================

pub use crate::backend::common::{
    AsmBinaryOp, AsmInstr, AsmOperand, AsmType, AsmUnaryOp, AsmX87BinaryOp, CondCode, Reg, XmmReg,
};

#[derive(Debug)]
pub struct AsmFunction {
    pub name: String,
    pub global: bool,
    pub instructions: Vec<AsmInstr>,
}

#[derive(Debug)]
pub struct AsmStaticVar {
    pub name: String,
    pub global: bool,
    pub thread_local: bool,
    pub alignment: usize,
    pub init_values: Vec<StaticInit>,
}

#[derive(Debug)]
pub struct AsmStaticConstant {
    pub name: String,
    pub alignment: usize,
    pub init: StaticInit,
}

#[derive(Debug)]
pub enum AsmTopLevel {
    Function(AsmFunction),
    StaticVar(AsmStaticVar),
    StaticConstant(AsmStaticConstant),
    Alias { name: String, target: String },
}

#[derive(Debug)]
pub struct AsmProgram {
    pub top_level: Vec<AsmTopLevel>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_string_byte_helpers_preserve_byte_semantics() {
        let value = "A\u{00e9}\u{20ac}";
        let bytes = c_string_bytes(value);
        assert_eq!(bytes, vec![b'A', 0xe9, 0xe2, 0x82, 0xac]);
        assert_eq!(c_string_byte_len(value), bytes.len());
        assert!(!c_string_contains_zero(value));
        assert!(c_string_contains_zero("a\0b"));
        assert_eq!(c_string_truncate_bytes(value, 2), "A\u{00e9}");
        assert_eq!(c_string_truncate_bytes(value, 3), "A\u{00e9}");
        assert_eq!(c_string_truncate_bytes(value, 4), "A\u{00e9}");
        assert_eq!(c_string_truncate_bytes(value, 5), value);
    }

    #[test]
    fn host_target_matches_compilation_platform() {
        let host = Target::host();
        assert_eq!(
            host.os,
            if cfg!(target_os = "macos") {
                TargetOs::MacOs
            } else {
                TargetOs::Linux
            }
        );
        assert_eq!(
            host.arch,
            if cfg!(target_arch = "aarch64") {
                Arch::AArch64
            } else {
                Arch::X86_64
            }
        );
    }

    #[test]
    fn stage_output_rules_are_centralized() {
        assert!(Stage::Preprocess.accepts_output());
        assert!(Stage::Assembly.accepts_output());
        assert!(Stage::Object.accepts_output());
        assert!(Stage::Executable.accepts_output());
        assert!(!Stage::Lex.accepts_output());
        assert!(Stage::Assembly.output_requires_single_input());
        assert!(!Stage::Executable.output_requires_single_input());
    }

    #[test]
    fn show_label_preserves_ascii_symbol_names() {
        assert_eq!(Target::x86_64_linux().show_symbol("main"), "main");
        assert_eq!(
            Target::x86_64_linux().show_symbol("__double_const_0"),
            "__double_const_0"
        );
        assert_eq!(Target::x86_64_macos().show_symbol("main"), "_main");
    }

    #[test]
    fn show_label_mangles_unicode_symbol_names() {
        assert_eq!(
            Target::x86_64_linux().show_symbol("αβ_global"),
            "__rnqcc_u_x3b1__x3b2___global_h80d54a5cf5297ffe"
        );
        assert_eq!(
            Target::x86_64_macos().show_symbol("αβ_global"),
            "___rnqcc_u_x3b1__x3b2___global_h80d54a5cf5297ffe"
        );
    }

    #[test]
    fn show_data_label_expr_preserves_data_offsets() {
        assert_eq!(
            Target::x86_64_linux().show_data_label_expr("origin+4"),
            "origin+4"
        );
        assert_eq!(
            Target::x86_64_linux().show_data_label_expr("origin-4"),
            "origin-4"
        );
        assert_eq!(
            Target::x86_64_linux().show_data_label_expr("αβ_global+4"),
            "__rnqcc_u_x3b1__x3b2___global_h80d54a5cf5297ffe+4"
        );
        assert_eq!(
            Target::x86_64_linux().show_data_label_expr("αβ_global-4"),
            "__rnqcc_u_x3b1__x3b2___global_h80d54a5cf5297ffe-4"
        );
        assert_eq!(
            Target::x86_64_macos().show_data_label_expr("αβ_global+4"),
            "___rnqcc_u_x3b1__x3b2___global_h80d54a5cf5297ffe+4"
        );
        assert_eq!(
            Target::x86_64_linux().show_symbol_with_offset("αβ_global", -4),
            "__rnqcc_u_x3b1__x3b2___global_h80d54a5cf5297ffe-4"
        );
    }

    #[test]
    fn show_label_does_not_collide_with_ascii_lookalikes() {
        assert_eq!(
            Target::x86_64_linux().show_symbol("__rnqcc_u_x3b1__x3b2___global_h80d54a5cf5297ffe"),
            "__rnqcc_u____rnqcc__u__x3b1____x3b2______global__h80d54a5cf5297ffe_h96ccdba34d0da6e9"
        );
        assert_ne!(
            Target::x86_64_linux().show_symbol("αβ_global"),
            Target::x86_64_linux().show_symbol("__rnqcc_u_x3b1__x3b2___global_h80d54a5cf5297ffe")
        );
    }

    #[test]
    fn union_classification_merges_integer_over_sse() -> Result<(), String> {
        let members = vec![
            MemberDeclaration {
                name: "d".to_string(),
                member_type: CType::Double,
                member_full_type: FullType::Scalar(CType::Double),
                bit_width: None,
                flexible_array: false,
                alignment: None,
                packed: false,
            },
            MemberDeclaration {
                name: "l".to_string(),
                member_type: CType::Long,
                member_full_type: FullType::Scalar(CType::Long),
                bit_width: None,
                flexible_array: false,
                alignment: None,
                packed: false,
            },
        ];
        let union = StructDef::from_members_union("U", &members, &IndexMap::new())?;

        assert_eq!(union.size, 8);
        assert_eq!(union.alignment, 8);
        assert!(union.is_union);
        assert_eq!(
            union.classify_with(&IndexMap::new()),
            vec![ParamClass::Integer]
        );
        Ok(())
    }

    #[test]
    fn union_classification_flattens_arrays_of_structs() -> Result<(), String> {
        let child_members = vec![MemberDeclaration {
            name: "d".to_string(),
            member_type: CType::Double,
            member_full_type: FullType::Scalar(CType::Double),
            bit_width: None,
            flexible_array: false,
            alignment: None,
            packed: false,
        }];
        let child = StructDef::from_members("Child", &child_members, &IndexMap::new())?;
        let mut defs = IndexMap::new();
        defs.insert("Child".to_string(), child);
        let union_members = vec![MemberDeclaration {
            name: "items".to_string(),
            member_type: CType::Struct,
            member_full_type: FullType::Array {
                elem: Box::new(FullType::Struct("Child".to_string())),
                size: 2,
            },
            bit_width: None,
            flexible_array: false,
            alignment: None,
            packed: false,
        }];
        let union = StructDef::from_members_union("U", &union_members, &defs)?;

        assert_eq!(
            union.classify_with(&defs),
            vec![ParamClass::Sse, ParamClass::Sse]
        );
        Ok(())
    }

    #[test]
    fn struct_classification_treats_float_as_sse_and_integer_dominates() -> Result<(), String> {
        let float_member = MemberDeclaration {
            name: "f".to_string(),
            member_type: CType::Float,
            member_full_type: FullType::Scalar(CType::Float),
            bit_width: None,
            flexible_array: false,
            alignment: None,
            packed: false,
        };
        let float_struct = StructDef::from_members(
            "FloatOnly",
            std::slice::from_ref(&float_member),
            &IndexMap::new(),
        )?;
        assert_eq!(float_struct.classify(), vec![ParamClass::Sse]);

        let int_member = MemberDeclaration {
            name: "i".to_string(),
            member_type: CType::Int,
            member_full_type: FullType::Scalar(CType::Int),
            bit_width: None,
            flexible_array: false,
            alignment: None,
            packed: false,
        };
        let mixed_struct =
            StructDef::from_members("FloatAndInt", &[float_member, int_member], &IndexMap::new())?;
        assert_eq!(mixed_struct.classify(), vec![ParamClass::Integer]);
        Ok(())
    }

    #[test]
    fn struct_classification_passes_unaligned_packed_fields_in_memory() -> Result<(), String> {
        let members = [
            MemberDeclaration {
                name: "prefix".to_string(),
                member_type: CType::Char,
                member_full_type: FullType::Scalar(CType::Char),
                bit_width: None,
                flexible_array: false,
                alignment: None,
                packed: false,
            },
            MemberDeclaration {
                name: "value".to_string(),
                member_type: CType::Double,
                member_full_type: FullType::Scalar(CType::Double),
                bit_width: None,
                flexible_array: false,
                alignment: None,
                packed: true,
            },
        ];
        let packed = StructDef::from_members("Packed", &members, &IndexMap::new())?;

        assert_eq!(packed.members[1].offset, 1);
        assert_eq!(packed.size, 9);
        assert_eq!(packed.classify(), vec![ParamClass::Memory]);
        Ok(())
    }

    #[test]
    fn struct_classification_flattens_vector_lanes() -> Result<(), String> {
        let member = MemberDeclaration {
            name: "v".to_string(),
            member_type: CType::Float,
            member_full_type: FullType::Vector {
                elem: Box::new(FullType::Scalar(CType::Float)),
                lanes: 4,
                complex: false,
            },
            bit_width: None,
            flexible_array: false,
            alignment: None,
            packed: false,
        };
        let vector_struct = StructDef::from_members("Vector", &[member], &IndexMap::new())?;

        assert_eq!(vector_struct.size, 16);
        assert_eq!(
            vector_struct.classify(),
            vec![ParamClass::Sse, ParamClass::Sse]
        );
        Ok(())
    }

    #[test]
    fn abi_flattening_rejects_overflowing_member_offsets() {
        let member = StructMember {
            name: "overflow".to_string(),
            member_type: CType::Double,
            member_full_type: FullType::Scalar(CType::Double),
            flexible_array: false,
            offset: usize::MAX,
            size: 8,
            bit_offset: 0,
            bit_width: None,
            reverse_storage_order: false,
        };
        let def = StructDef {
            tag: "Overflow".to_string(),
            members: vec![member],
            size: 8,
            alignment: 8,
            is_union: false,
        };

        assert!(def.flatten_fields(1, &IndexMap::new()).is_empty());
    }

    #[test]
    fn abi_flattening_rejects_overflowing_struct_array_offsets() {
        let child = StructDef {
            tag: "Child".to_string(),
            members: vec![StructMember {
                name: "value".to_string(),
                member_type: CType::Double,
                member_full_type: FullType::Scalar(CType::Double),
                flexible_array: false,
                offset: 0,
                size: 8,
                bit_offset: 0,
                bit_width: None,
                reverse_storage_order: false,
            }],
            size: usize::MAX / 2,
            alignment: 8,
            is_union: false,
        };
        let member = StructMember {
            name: "children".to_string(),
            member_type: CType::Struct,
            member_full_type: FullType::Array {
                elem: Box::new(FullType::Struct("Child".to_string())),
                size: 2,
            },
            flexible_array: false,
            offset: 0,
            size: usize::MAX,
            bit_offset: 0,
            bit_width: None,
            reverse_storage_order: false,
        };
        let outer = StructDef {
            tag: "Outer".to_string(),
            members: vec![member],
            size: usize::MAX,
            alignment: 8,
            is_union: false,
        };
        let mut defs = IndexMap::new();
        defs.insert("Child".to_string(), child);

        assert_eq!(outer.flatten_fields(usize::MAX - 1, &defs).len(), 1);
    }

    #[test]
    fn struct_member_size_overflow_is_reported() {
        let member = MemberDeclaration {
            name: "huge".to_string(),
            member_type: CType::Int,
            member_full_type: FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Int)),
                size: usize::MAX,
            },
            bit_width: None,
            flexible_array: false,
            alignment: None,
            packed: false,
        };
        let error = StructDef::from_members("Huge", &[member], &IndexMap::new())
            .expect_err("overflowing array member size should be rejected");
        assert!(
            error.contains("struct member array size is too large"),
            "{error}"
        );
    }

    #[test]
    fn checked_full_type_size_rejects_array_overflow() {
        let ty = FullType::Array {
            elem: Box::new(FullType::Scalar(CType::Long)),
            size: usize::MAX,
        };
        assert_eq!(ty.checked_byte_size_with(&IndexMap::new()), None);
    }

    #[test]
    fn unchecked_full_type_size_saturates_on_overflow() {
        let ty = FullType::Array {
            elem: Box::new(FullType::Scalar(CType::Long)),
            size: usize::MAX,
        };
        assert_eq!(ty.byte_size(), usize::MAX);
        assert_eq!(ty.byte_size_with(&IndexMap::new()), usize::MAX);
    }
}
