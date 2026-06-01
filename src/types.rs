// ============================================================
// Target & Compiler Stage
// ============================================================
#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    AArch64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    MacOs,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub arch: Arch,
    pub os: TargetOs,
}

impl Target {
    pub const SUPPORTED: [Self; 4] = [
        Self::x86_64_linux(),
        Self::x86_64_macos(),
        Self::aarch64_linux(),
        Self::aarch64_macos(),
    ];
    pub const ALIASES: [(&'static str, Self); 14] = [
        ("linux", Self::x86_64_linux()),
        ("osx", Self::x86_64_macos()),
        ("macos", Self::x86_64_macos()),
        ("x86_64-linux", Self::x86_64_linux()),
        ("x86_64-unknown-linux-gnu", Self::x86_64_linux()),
        ("x86_64-osx", Self::x86_64_macos()),
        ("x86_64-macos", Self::x86_64_macos()),
        ("x86_64-apple-darwin", Self::x86_64_macos()),
        ("arm64-linux", Self::aarch64_linux()),
        ("aarch64-linux", Self::aarch64_linux()),
        ("aarch64-unknown-linux-gnu", Self::aarch64_linux()),
        ("arm64-macos", Self::aarch64_macos()),
        ("aarch64-macos", Self::aarch64_macos()),
        ("aarch64-apple-darwin", Self::aarch64_macos()),
    ];
    pub const fn x86_64_linux() -> Self {
        Target {
            arch: Arch::X86_64,
            os: TargetOs::Linux,
        }
    }

    pub const fn x86_64_macos() -> Self {
        Target {
            arch: Arch::X86_64,
            os: TargetOs::MacOs,
        }
    }

    pub const fn aarch64_linux() -> Self {
        Target {
            arch: Arch::AArch64,
            os: TargetOs::Linux,
        }
    }

    pub const fn aarch64_macos() -> Self {
        Target {
            arch: Arch::AArch64,
            os: TargetOs::MacOs,
        }
    }

    pub fn show_label(&self, name: &str) -> String {
        match self.os {
            TargetOs::MacOs => format!("_{}", name),
            TargetOs::Linux => name.to_string(),
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALIASES
            .iter()
            .find_map(|(alias, target)| (*alias == name).then_some(*target))
    }

    pub fn triple_name(&self) -> &'static str {
        match (self.arch, self.os) {
            (Arch::X86_64, TargetOs::Linux) => "x86_64-linux",
            (Arch::X86_64, TargetOs::MacOs) => "x86_64-macos",
            (Arch::AArch64, TargetOs::Linux) => "aarch64-linux",
            (Arch::AArch64, TargetOs::MacOs) => "aarch64-macos",
        }
    }

    pub fn host() -> Self {
        match (std::env::consts::ARCH, std::env::consts::OS) {
            ("aarch64", "macos") => Self::aarch64_macos(),
            ("x86_64", "macos") => Self::x86_64_macos(),
            ("aarch64", _) => Self::aarch64_linux(),
            _ => Self::x86_64_linux(),
        }
    }

    pub fn cc_arch_args(&self) -> Vec<&'static str> {
        match (self.arch, self.os) {
            (Arch::X86_64, TargetOs::MacOs) => vec!["-arch", "x86_64"],
            (Arch::AArch64, TargetOs::MacOs) => vec!["-arch", "arm64"],
            (_, TargetOs::Linux) => Vec::new(),
        }
    }

    pub fn can_use_host_driver(&self) -> bool {
        let host = Self::host();
        *self == host || (self.os == TargetOs::MacOs && host.os == TargetOs::MacOs)
    }

    pub fn long_double_size(&self) -> usize {
        match (self.arch, self.os) {
            (Arch::AArch64, TargetOs::MacOs) => 8,
            _ => 16,
        }
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

#[derive(Debug, Clone, Copy, PartialEq)]
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
    let mut out = Vec::new();
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
    c_string_bytes(s).len()
}

pub fn c_string_truncate_bytes(s: &str, max_bytes: usize) -> String {
    let mut out = String::new();
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

    /// Total byte size of this type (note: for Struct, returns 0 without struct_defs)
    pub fn byte_size(&self) -> usize {
        match self {
            FullType::Scalar(t) => std::cmp::max(t.size() as usize, 1),
            FullType::Pointer(_) => 8,
            FullType::Function { .. } => 8,
            FullType::Array { elem, size } => elem.byte_size() * size,
            FullType::Vector { elem, lanes, .. } => elem.byte_size() * lanes,
            FullType::Struct(_) => 0, // need struct_defs to compute; caller should use byte_size_with
        }
    }

    /// Total byte size with struct definitions
    pub fn byte_size_with(
        &self,
        struct_defs: &std::collections::HashMap<String, StructDef>,
    ) -> usize {
        match self {
            FullType::Struct(tag) => struct_defs.get(tag).map(|d| d.size).unwrap_or(0),
            FullType::Array { elem, size } => elem.byte_size_with(struct_defs) * size,
            FullType::Vector { elem, lanes, .. } => elem.byte_size_with(struct_defs) * lanes,
            _ => self.byte_size(),
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
    pub fn alignment_with(
        &self,
        struct_defs: &std::collections::HashMap<String, StructDef>,
    ) -> usize {
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
    /// Classify a struct for System V ABI parameter/return passing.
    /// Returns a list of ParamClass for each 8-byte chunk, or Memory if passed on stack.
    /// Flatten all fields to (byte_offset, scalar_type) pairs,
    /// recursing into nested structs and arrays.
    fn flatten_fields(
        &self,
        base_offset: usize,
        struct_defs: &std::collections::HashMap<String, StructDef>,
    ) -> Vec<(usize, CType)> {
        let mut fields = Vec::new();
        for mem in &self.members {
            let abs_offset = base_offset + mem.offset;
            match &mem.member_full_type {
                FullType::Struct(tag) => {
                    if let Some(def) = struct_defs.get(tag) {
                        fields.extend(def.flatten_fields(abs_offset, struct_defs));
                    }
                }
                FullType::Array { elem, size: _ } => {
                    let mut inner = elem.as_ref();
                    while let FullType::Array { elem: e, .. } = inner {
                        inner = e;
                    }
                    let elem_size = inner.byte_size();
                    let scalar_type = inner.to_ctype();
                    // For arrays of structs, recurse
                    if let FullType::Struct(tag) = inner {
                        if let Some(def) = struct_defs.get(tag) {
                            let total_elems: usize = mem.size / std::cmp::max(def.size, 1);
                            for i in 0..total_elems {
                                fields.extend(
                                    def.flatten_fields(abs_offset + i * def.size, struct_defs),
                                );
                            }
                        }
                    } else {
                        let total_elems = mem.size.checked_div(elem_size).unwrap_or(0);
                        for i in 0..total_elems {
                            fields.push((abs_offset + i * elem_size, scalar_type));
                        }
                    }
                }
                _ => {
                    fields.push((abs_offset, mem.member_type));
                }
            }
        }
        fields
    }

    fn flatten_member_fields(
        &self,
        mem: &StructMember,
        base_offset: usize,
        struct_defs: &std::collections::HashMap<String, StructDef>,
    ) -> Vec<(usize, CType)> {
        let abs_offset = base_offset + mem.offset;
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
                let scalar_type = inner.to_ctype();
                let elem_size = inner.byte_size();
                if elem_size == 0 {
                    return vec![];
                }
                let total_elems = mem.size / elem_size;
                (0..total_elems)
                    .map(|i| (abs_offset + i * elem_size, scalar_type))
                    .collect()
            }
            _ => vec![(abs_offset, mem.member_type)],
        }
    }

    pub fn classify_with(
        &self,
        struct_defs: &std::collections::HashMap<String, StructDef>,
    ) -> Vec<ParamClass> {
        if self.size > 16 {
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
                            let has_double = fields
                                .iter()
                                .any(|(off, ct)| off / 8 == eb && *ct == CType::Double);
                            mc.push(if has_double {
                                ParamClass::Sse
                            } else {
                                ParamClass::Integer
                            });
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
            let mut classes = vec![ParamClass::Integer; num_eightbytes];
            let fields = self.flatten_fields(0, struct_defs);
            for (offset, ctype) in &fields {
                if *ctype == CType::Double {
                    let eb = offset / 8;
                    if eb < num_eightbytes {
                        classes[eb] = ParamClass::Sse;
                    }
                }
            }
            classes
        }
    }

    pub fn classify(&self) -> Vec<ParamClass> {
        // Legacy version without struct_defs — works for structs without nested structs
        self.classify_with(&std::collections::HashMap::new())
    }
}

#[derive(Debug, Clone)]
pub struct StructMember {
    pub name: String,
    pub member_type: CType,
    pub member_full_type: FullType,
    pub offset: usize,
    pub size: usize,
    pub bit_width: Option<u8>,
    pub bit_offset: u8,
    pub reverse_storage_order: bool,
}

impl StructDef {
    pub fn from_declaration(
        declaration: &StructDeclaration,
        struct_defs: &std::collections::HashMap<String, StructDef>,
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
        struct_defs: &std::collections::HashMap<String, StructDef>,
    ) -> Result<Self, String> {
        Self::from_members_ex(tag, members, struct_defs, false, false, None, false)
    }

    pub fn from_members_packed(
        tag: &str,
        members: &[MemberDeclaration],
        struct_defs: &std::collections::HashMap<String, StructDef>,
    ) -> Result<Self, String> {
        Self::from_members_ex(tag, members, struct_defs, false, true, None, false)
    }

    pub fn from_members_aligned(
        tag: &str,
        members: &[MemberDeclaration],
        struct_defs: &std::collections::HashMap<String, StructDef>,
        alignment: std::num::NonZeroUsize,
    ) -> Result<Self, String> {
        Self::from_members_ex(
            tag,
            members,
            struct_defs,
            false,
            false,
            Some(alignment),
            false,
        )
    }

    pub fn from_members_packed_aligned(
        tag: &str,
        members: &[MemberDeclaration],
        struct_defs: &std::collections::HashMap<String, StructDef>,
        alignment: std::num::NonZeroUsize,
    ) -> Result<Self, String> {
        Self::from_members_ex(
            tag,
            members,
            struct_defs,
            false,
            true,
            Some(alignment),
            false,
        )
    }

    pub fn from_members_union(
        tag: &str,
        members: &[MemberDeclaration],
        struct_defs: &std::collections::HashMap<String, StructDef>,
    ) -> Result<Self, String> {
        Self::from_members_ex(tag, members, struct_defs, true, false, None, false)
    }

    pub fn from_members_union_packed(
        tag: &str,
        members: &[MemberDeclaration],
        struct_defs: &std::collections::HashMap<String, StructDef>,
    ) -> Result<Self, String> {
        Self::from_members_ex(tag, members, struct_defs, true, true, None, false)
    }

    pub fn from_members_union_aligned(
        tag: &str,
        members: &[MemberDeclaration],
        struct_defs: &std::collections::HashMap<String, StructDef>,
        alignment: std::num::NonZeroUsize,
    ) -> Result<Self, String> {
        Self::from_members_ex(
            tag,
            members,
            struct_defs,
            true,
            false,
            Some(alignment),
            false,
        )
    }

    pub fn from_members_union_packed_aligned(
        tag: &str,
        members: &[MemberDeclaration],
        struct_defs: &std::collections::HashMap<String, StructDef>,
        alignment: std::num::NonZeroUsize,
    ) -> Result<Self, String> {
        Self::from_members_ex(
            tag,
            members,
            struct_defs,
            true,
            true,
            Some(alignment),
            false,
        )
    }

    fn from_members_ex(
        tag: &str,
        members: &[MemberDeclaration],
        struct_defs: &std::collections::HashMap<String, StructDef>,
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
                let storage_bits = m_size * 8;
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
                let storage_bits = storage_size * 8;
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

                let needs_new_unit = bit_unit_size == 0
                    || bit_unit_size != storage_size
                    || bit_unit_align != storage_align
                    || next_bit_offset + width as usize > storage_bits;
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
                next_bit_offset += width as usize;
                max_align = max_align.max(storage_align);
                continue;
            }

            if next_bit_offset > 0 {
                offset = bit_unit_offset + next_bit_offset.div_ceil(8);
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
    struct_defs: &std::collections::HashMap<String, StructDef>,
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
            let total = elem_size * size;
            // Inside structs, array alignment is just the element alignment
            Ok((total, elem_align))
        }
        FullType::Vector { elem, lanes, .. } => {
            let (elem_size, elem_align) = member_size_align(elem, struct_defs)?;
            Ok((elem_size * lanes, elem_align))
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
    ImaginaryIntLiteral(i64),
    ImaginaryDoubleLiteral(f64),
    CharLiteral(i64),
    StringLiteral(String),
    WideStringLiteral(String),
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
    AttributeAligned(String),
    AttributeAlignedNoreturn(String),
    AttributePacked,
    AttributePackedAligned(String),
    AttributePackedAlignedNoreturn(String),
    AttributeTransparentUnion,
    AttributeNoreturn,
    AttributeNoInstrumentFunction,
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
    ImaginaryIntConstant(i64),
    ImaginaryDoubleConstant(f64),
    StringLiteral(String),
    WideStringLiteral(String),
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
    pub value: Option<i64>, // None = default
    pub end_value: Option<i64>,
    pub label: String,
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
    pub fn is_static(&self) -> bool {
        matches!(self, Self::Static | Self::StaticThreadLocal)
    }

    pub fn is_extern(&self) -> bool {
        matches!(self, Self::Extern | Self::ExternThreadLocal)
    }

    pub fn is_typedef(&self) -> bool {
        matches!(self, Self::Typedef)
    }

    pub fn is_thread_local(&self) -> bool {
        matches!(
            self,
            Self::ThreadLocal | Self::StaticThreadLocal | Self::ExternThreadLocal
        )
    }

    pub fn with_static(self) -> Self {
        match self {
            Self::ThreadLocal => Self::StaticThreadLocal,
            other => other,
        }
    }

    pub fn with_extern(self) -> Self {
        match self {
            Self::ThreadLocal => Self::ExternThreadLocal,
            other => other,
        }
    }

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

#[derive(Debug, Clone, PartialEq)]
pub enum TackyVal {
    Constant(i64),
    Int128Constant(i128),
    UInt128Constant(u128),
    DoubleConstant(f64),
    Var(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TackyUnaryOp {
    Negate,
    Complement,
    LogicalNot,
}

#[derive(Debug, Clone, PartialEq)]
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
        /// Stack-passed aggregate blocks: (flattened arg index containing source address, byte size).
        memory_arg_blocks: Vec<(usize, usize)>,
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

#[derive(Debug)]
pub struct TackyFunction {
    pub name: String,
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

#[derive(Debug)]
pub struct TackyStaticVar {
    pub name: String,
    pub global: bool,
    pub thread_local: bool,
    pub alignment: usize,
    pub init_values: Vec<StaticInit>,
}

#[derive(Debug)]
pub struct TackyStaticConstant {
    pub name: String,
    pub alignment: usize,
    pub init: StaticInit,
}

#[derive(Debug)]
pub enum TackyTopLevel {
    Function(TackyFunction),
    StaticVar(TackyStaticVar),
    StaticConstant(TackyStaticConstant),
    Alias { name: String, target: String },
}

#[derive(Debug)]
pub struct TackyProgram {
    pub top_level: Vec<TackyTopLevel>,
    pub global_vars: std::collections::HashSet<String>,
    pub thread_local_vars: std::collections::HashSet<String>,
    pub symbol_types: std::collections::HashMap<String, CType>,
    pub symbol_alignments: std::collections::HashMap<String, usize>,
    /// Array/struct storage sizes
    pub array_sizes: std::collections::HashMap<String, usize>,
    /// Struct definitions for ABI classification
    pub struct_defs: std::collections::HashMap<String, StructDef>,
    /// Map from variable name to struct tag
    pub var_struct_tags: std::collections::HashMap<String, String>,
}

// ============================================================
// Assembly IR
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AsmType {
    Byte,       // 1-byte char
    Word,       // 16-bit short
    Longword,   // 32-bit int
    Quadword,   // 64-bit long
    Octword,    // 128-bit integer
    Float,      // 32-bit float (XMM)
    Double,     // 64-bit float (XMM)
    LongDouble, // target long double storage: x87 extended or binary128
}

impl From<CType> for AsmType {
    fn from(t: CType) -> Self {
        match t {
            CType::Char | CType::SChar | CType::UChar | CType::Bool => AsmType::Byte,
            CType::Short | CType::UShort => AsmType::Word,
            CType::Int | CType::UInt => AsmType::Longword,
            CType::Long | CType::ULong | CType::Pointer => AsmType::Quadword,
            CType::Int128 | CType::UInt128 => AsmType::Octword,
            CType::Float => AsmType::Float,
            CType::Double => AsmType::Double,
            CType::LongDouble => AsmType::LongDouble,
            CType::Void => AsmType::Longword,
            CType::Struct => AsmType::Longword, // struct size tracked separately
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum XmmReg {
    XMM0,
    XMM1,
    XMM2,
    XMM3,
    XMM4,
    XMM5,
    XMM6,
    XMM7,
    XMM8,
    XMM9,
    XMM10,
    XMM11,
    XMM12,
    XMM13,
    XMM14,
    XMM15,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reg {
    AX,
    BX,
    CX,
    DX,
    DI,
    SI,
    R8,
    R9,
    R10,
    R11,
    R12,
    R13,
    R14,
    R15,
    SP,
    BP,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AsmOperand {
    Imm(i64),
    Reg(Reg),
    Xmm(XmmReg),
    Pseudo(String),
    /// Aggregate object at byte offset (for arrays/structs)
    PseudoMem(String, i32),
    Stack(i32),
    StackArg(i32),
    Data(String),
    TlsData(String, i32),
    /// Indexed addressing: base_reg + index_reg * scale
    Indexed(Reg, Reg, i32),
}

#[derive(Debug, Clone)]
pub enum AsmUnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone)]
pub enum AsmBinaryOp {
    Add,
    AddSetFlags,
    Adc,
    Sub,
    SubSetFlags,
    Sbb,
    Mul,
    SDiv,
    UDiv,
    DivDouble, // divsd (double division only)
    And,
    Nand,
    Or,
    Xor,
    Sal,
    Sar,
    Shr,
}

#[derive(Debug, Clone)]
pub enum AsmX87BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone)]
pub enum CondCode {
    E,
    NE,
    L,
    LE,
    G,
    GE,
    // Unsigned
    A,  // above
    AE, // above or equal
    B,  // below
    BE, // below or equal
}

#[derive(Debug, Clone)]
pub enum AsmInstr {
    Mov(AsmType, AsmOperand, AsmOperand),
    Movsx(AsmType, AsmType, AsmOperand, AsmOperand), // (src_type, dst_type, src, dst) sign-extend
    MovZeroExtend(AsmType, AsmType, AsmOperand, AsmOperand), // (src_type, dst_type, src, dst) zero-extend
    Unary(AsmType, AsmUnaryOp, AsmOperand),
    Binary(AsmType, AsmBinaryOp, AsmOperand, AsmOperand),
    MulFull(AsmType, AsmOperand), // RDX:RAX = RAX * operand
    Idiv(AsmType, AsmOperand),
    Div(AsmType, AsmOperand), // unsigned division
    Cdq(AsmType),             // Longword=cdq, Quadword=cqo
    Cmp(AsmType, AsmOperand, AsmOperand),
    Jmp(String),
    NonlocalJmp(String),
    JmpIndirect(AsmOperand),
    JmpCC(CondCode, String),
    SetCC(CondCode, AsmOperand),
    Label(String),
    LoadLabelAddress(String, AsmOperand),
    BuiltinSetjmp {
        buf: AsmOperand,
        dst: AsmOperand,
        label: String,
        end_label: String,
    },
    BuiltinLongjmp {
        buf: AsmOperand,
        value: AsmOperand,
    },
    Push(AsmOperand),
    Call(String, usize, usize, bool), // name, int_reg_args, sse_reg_args, indirect
    Pop(Reg),
    Cvtsi2sd(AsmType, AsmOperand, AsmOperand), // int/long → double
    Cvtsi2ss(AsmType, AsmOperand, AsmOperand), // int/long → float
    Cvttsd2si(AsmType, AsmOperand, AsmOperand), // double → int/long (truncate)
    Cvttss2si(AsmType, AsmOperand, AsmOperand), // float → int/long (truncate)
    Cvtss2sd(AsmOperand, AsmOperand),          // float → double
    Cvtsd2ss(AsmOperand, AsmOperand),          // double → float
    X87Load(AsmType, AsmOperand),
    X87Store(AsmOperand),
    X87LoadIndirect(AsmType, Reg),
    X87StoreIndirect(Reg),
    X87UnaryNeg,
    X87Binary(AsmX87BinaryOp),
    X87Compare,
    /// AArch64-only unsigned integer to double conversion.
    AArch64UIntToDouble(AsmType, AsmOperand, AsmOperand), // src_type, src, dst
    /// AArch64-only unsigned integer to float conversion.
    AArch64UIntToFloat(AsmType, AsmOperand, AsmOperand), // src_type, src, dst
    /// AArch64-only double to unsigned integer conversion.
    AArch64DoubleToUInt(AsmType, AsmOperand, AsmOperand), // dst_type, src, dst
    /// AArch64-only float to unsigned integer conversion.
    AArch64FloatToUInt(AsmType, AsmOperand, AsmOperand), // dst_type, src, dst
    /// AArch64-only float/double conversion.
    AArch64FloatToDouble(AsmOperand, AsmOperand),
    /// AArch64-only double/float conversion.
    AArch64DoubleToFloat(AsmOperand, AsmOperand),
    /// x86-64 SysV varargs call metadata: write the XMM argument count to %al.
    X86SetVarargsXmmCount(usize),
    AtomicFence,
    AtomicRmw(AsmType, AsmBinaryOp, bool, AsmOperand),
    AtomicExchange(AsmType, AsmOperand),
    AtomicCompareExchange(AsmType, AsmOperand),
    AtomicCompareSwap(AsmType, bool, AsmOperand),
    Lea(AsmOperand, AsmOperand), // leaq src, dst
    /// Load from memory pointed to by a register: mov (reg), dst
    LoadIndirect(AsmType, Reg, AsmOperand),
    /// Store to memory pointed to by a register: mov src, (reg)
    StoreIndirect(AsmType, AsmOperand, Reg),
    /// Copy `size` bytes from pointer operand to outgoing call stack at `%rsp + dst_offset`.
    CopyToStackArg {
        src_ptr: AsmOperand,
        dst_offset: i32,
        size: usize,
    },
    /// Copy `size` bytes from incoming call stack at `%rbp + src_offset` to aggregate storage.
    CopyFromStackArg {
        src_offset: i32,
        dst: AsmOperand,
        size: usize,
    },
    /// AArch64-only pointer addition: dst = ptr + index * scale.
    AArch64AddPtr(AsmOperand, AsmOperand, i64, AsmOperand), // ptr, index, scale, dst
    /// AArch64-only load after temporary stack allocation rebases local stack operands.
    AArch64LoadAdjusted(AsmType, AsmOperand, Reg, i32), // type, src, dst register, local rebase
    /// AArch64-only outgoing call argument store after temporary stack allocation.
    AArch64StoreOutgoingArg(AsmType, AsmOperand, i32, i32), // type, src, outgoing offset, local rebase
    /// AArch64-only integer remainder: dst = left % right.
    AArch64Rem(AsmType, bool, AsmOperand, AsmOperand, AsmOperand), // type, unsigned, left, right, dst
    /// AArch64-only save/restore of the link register in non-leaf functions.
    AArch64SaveLink(i32), // stack offset from sp
    AArch64RestoreLink(i32), // stack offset from sp
    Unreachable,
    Ret,
    AllocateStack(i32),
    DeallocateStack(i32),
}

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
    use std::collections::HashMap;

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
        let union = StructDef::from_members_union("U", &members, &HashMap::new())?;

        assert_eq!(union.size, 8);
        assert_eq!(union.alignment, 8);
        assert!(union.is_union);
        assert_eq!(
            union.classify_with(&HashMap::new()),
            vec![ParamClass::Integer]
        );
        Ok(())
    }
}
