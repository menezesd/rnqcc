// ============================================================
// Platform & Compiler Stage
// ============================================================

#[derive(Debug, PartialEq)]
pub enum Platform {
    OsX,
    Linux,
}

impl Platform {
    pub fn show_label(&self, name: &str) -> String {
        match self {
            Platform::OsX => format!("_{}", name),
            Platform::Linux => name.to_string(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Stage {
    Lex,
    Parse,
    Validate,
    Tacky,
    Codegen,
    Assembly,
    Object,
    Executable,
}

// ============================================================
// C Types
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CType {
    Char,
    SChar,
    UChar,
    Int,
    Long,
    UInt,
    ULong,
    Double,
    /// Struct type (tag tracked separately via FullType)
    Struct,
    /// Pointer to some type. We don't track the pointee type at the assembly level —
    /// all pointers are 8 bytes. The pointee type is only needed for type checking
    /// which we handle during parsing/TACKY generation.
    Pointer,
    Void,
}

impl CType {
    pub fn size(self) -> i32 {
        match self {
            CType::Char | CType::SChar | CType::UChar => 1,
            CType::Int | CType::UInt => 4,
            CType::Long | CType::ULong | CType::Double | CType::Pointer => 8,
            CType::Void => 0,
            CType::Struct => 0, // size tracked via FullType/StructDef
        }
    }

    pub fn is_signed(self) -> bool {
        matches!(self, CType::Char | CType::SChar | CType::Int | CType::Long)
    }

    pub fn is_char(self) -> bool {
        matches!(self, CType::Char | CType::SChar | CType::UChar)
    }

    pub fn is_struct(self) -> bool {
        self == CType::Struct
    }

    pub fn is_double(self) -> bool {
        self == CType::Double
    }

    pub fn is_pointer(self) -> bool {
        self == CType::Pointer
    }

    /// Integer promotion: char types promote to Int
    pub fn promote(self) -> CType {
        if self.is_char() { CType::Int } else { self }
    }

    /// Usual arithmetic conversions (C standard 6.3.1.8)
    pub fn common(a: CType, b: CType) -> CType {
        // Integer promotions first
        let a = a.promote();
        let b = b.promote();
        if a == b { return a; }
        if a == CType::Double { return CType::Double; }
        if b == CType::Double { return CType::Double; }
        if a == CType::Pointer { return CType::Pointer; }
        if b == CType::Pointer { return CType::Pointer; }
        if a.size() == b.size() {
            if a.is_signed() { return b; } else { return a; }
        }
        if a.size() > b.size() { a } else { b }
    }
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
    Array { elem: Box<FullType>, size: usize },
    Struct(String), // struct tag name (resolved to unique identifier)
}

impl FullType {
    /// Convert to CType for codegen (arrays become ByteArray info, pointers become Pointer)
    pub fn to_ctype(&self) -> CType {
        match self {
            FullType::Scalar(t) => *t,
            FullType::Pointer(_) => CType::Pointer,
            FullType::Array { .. } => CType::Pointer, // arrays decay to pointers in most contexts
            FullType::Struct(_) => CType::Struct,
        }
    }

    /// Total byte size of this type (note: for Struct, returns 0 without struct_defs)
    pub fn byte_size(&self) -> usize {
        match self {
            FullType::Scalar(t) => std::cmp::max(t.size() as usize, 1),
            FullType::Pointer(_) => 8,
            FullType::Array { elem, size } => elem.byte_size() * size,
            FullType::Struct(_) => 0, // need struct_defs to compute; caller should use byte_size_with
        }
    }

    /// Total byte size with struct definitions
    pub fn byte_size_with(&self, struct_defs: &std::collections::HashMap<String, StructDef>) -> usize {
        match self {
            FullType::Struct(tag) => {
                struct_defs.get(tag).map(|d| d.size).unwrap_or(0)
            }
            FullType::Array { elem, size } => elem.byte_size_with(struct_defs) * size,
            _ => self.byte_size(),
        }
    }

    /// Alignment requirement
    pub fn alignment(&self) -> usize {
        match self {
            FullType::Scalar(t) => std::cmp::max(t.size() as usize, 1),
            FullType::Pointer(_) => 8,
            FullType::Array { elem, .. } => {
                let ea = elem.alignment();
                if self.byte_size() >= 16 { std::cmp::max(ea, 16) } else { ea }
            }
            FullType::Struct(_) => 1, // need struct_defs; caller should use alignment_with
        }
    }

    /// Get the element type (for arrays: inner element; for pointers: pointee)
    pub fn elem_type(&self) -> Option<&FullType> {
        match self {
            FullType::Array { elem, .. } => Some(elem),
            FullType::Pointer(inner) => Some(inner),
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
    pub fn from_decl(base: CType, ptr_info: Option<(CType, usize)>, array_dims: &Option<Vec<usize>>) -> FullType {
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
            let mut t = if ptr_info.is_some() { base_full } else { FullType::Scalar(base) };
            for &dim in dims.iter().rev() {
                if dim > 0 {
                    t = FullType::Array { elem: Box::new(t), size: dim };
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
    fn flatten_fields(&self, base_offset: usize, struct_defs: &std::collections::HashMap<String, StructDef>) -> Vec<(usize, CType)> {
        let mut fields = Vec::new();
        for mem in &self.members {
            let abs_offset = base_offset + mem.offset;
            match &mem.member_full_type {
                FullType::Struct(tag) => {
                    if let Some(def) = struct_defs.get(tag) {
                        fields.extend(def.flatten_fields(abs_offset, struct_defs));
                    }
                }
                FullType::Array { elem, size } => {
                    let mut inner = elem.as_ref();
                    while let FullType::Array { elem: e, .. } = inner { inner = e; }
                    let elem_size = inner.byte_size();
                    let scalar_type = inner.to_ctype();
                    // For arrays of structs, recurse
                    if let FullType::Struct(tag) = inner {
                        if let Some(def) = struct_defs.get(tag) {
                            let total_elems: usize = mem.size / std::cmp::max(def.size, 1);
                            for i in 0..total_elems {
                                fields.extend(def.flatten_fields(abs_offset + i * def.size, struct_defs));
                            }
                        }
                    } else {
                        let total_elems = if elem_size > 0 { mem.size / elem_size } else { 0 };
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

    fn flatten_member_fields(&self, mem: &StructMember, base_offset: usize, struct_defs: &std::collections::HashMap<String, StructDef>) -> Vec<(usize, CType)> {
        let abs_offset = base_offset + mem.offset;
        match &mem.member_full_type {
            FullType::Struct(tag) => {
                if let Some(def) = struct_defs.get(tag) {
                    def.flatten_fields(abs_offset, struct_defs)
                } else { vec![] }
            }
            FullType::Array { elem, .. } => {
                let mut inner = elem.as_ref();
                while let FullType::Array { elem: e, .. } = inner { inner = e; }
                let scalar_type = inner.to_ctype();
                let elem_size = inner.byte_size();
                if elem_size == 0 { return vec![]; }
                let total_elems = mem.size / elem_size;
                (0..total_elems).map(|i| (abs_offset + i * elem_size, scalar_type)).collect()
            }
            _ => vec![(abs_offset, mem.member_type)],
        }
    }

    pub fn classify_with(&self, struct_defs: &std::collections::HashMap<String, StructDef>) -> Vec<ParamClass> {
        if self.size > 16 {
            return vec![ParamClass::Memory];
        }
        let num_eightbytes = (self.size + 7) / 8;

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
                        let mem_ebs = (mem.size + 7) / 8;
                        let mut mc = Vec::new();
                        for eb in 0..std::cmp::min(mem_ebs, num_eightbytes) {
                            let has_double = fields.iter().any(|(off, ct)| off / 8 == eb && *ct == CType::Double);
                            mc.push(if has_double { ParamClass::Sse } else { ParamClass::Integer });
                        }
                        mc
                    }
                };
                // Merge: INTEGER dominates SSE
                for (eb, mc) in mem_classes.iter().enumerate() {
                    if eb >= num_eightbytes { break; }
                    match (&classes[eb], mc) {
                        (None, _) => classes[eb] = Some(*mc),
                        (Some(ParamClass::Integer), _) => {} // INTEGER stays
                        (Some(ParamClass::Sse), ParamClass::Integer) => classes[eb] = Some(ParamClass::Integer),
                        (Some(ParamClass::Sse), ParamClass::Sse) => {} // SSE stays
                        _ => {}
                    }
                }
            }
            classes.iter().map(|c| c.unwrap_or(ParamClass::Integer)).collect()
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
}

impl StructDef {
    /// Compute layout from member declarations
    pub fn from_members(tag: &str, members: &[MemberDeclaration], struct_defs: &std::collections::HashMap<String, StructDef>) -> Self {
        Self::from_members_ex(tag, members, struct_defs, false)
    }

    pub fn from_members_union(tag: &str, members: &[MemberDeclaration], struct_defs: &std::collections::HashMap<String, StructDef>) -> Self {
        Self::from_members_ex(tag, members, struct_defs, true)
    }

    fn from_members_ex(tag: &str, members: &[MemberDeclaration], struct_defs: &std::collections::HashMap<String, StructDef>, is_union: bool) -> Self {
        let mut offset = 0usize;
        let mut max_align = 1usize;
        let mut max_size = 0usize;
        let mut laid_out = Vec::new();

        for m in members {
            let (m_size, m_align) = member_size_align(&m.member_full_type, struct_defs);
            if is_union {
                // Union: all members at offset 0
                laid_out.push(StructMember {
                    name: m.name.clone(),
                    member_type: m.member_type,
                    member_full_type: m.member_full_type.clone(),
                    offset: 0,
                    size: m_size,
                });
                if m_size > max_size { max_size = m_size; }
            } else {
                // Struct: sequential layout with alignment
                offset = (offset + m_align - 1) & !(m_align - 1);
                laid_out.push(StructMember {
                    name: m.name.clone(),
                    member_type: m.member_type,
                    member_full_type: m.member_full_type.clone(),
                    offset,
                    size: m_size,
                });
                offset += m_size;
            }
            if m_align > max_align { max_align = m_align; }
        }

        let total_size = if is_union {
            // Union size = max member size, padded to alignment
            (max_size + max_align - 1) & !(max_align - 1)
        } else {
            (offset + max_align - 1) & !(max_align - 1)
        };

        StructDef {
            tag: tag.to_string(),
            members: laid_out,
            size: total_size,
            alignment: max_align,
            is_union,
        }
    }

    pub fn find_member(&self, name: &str) -> Option<&StructMember> {
        self.members.iter().find(|m| m.name == name)
    }
}

fn member_size_align(ft: &FullType, struct_defs: &std::collections::HashMap<String, StructDef>) -> (usize, usize) {
    match ft {
        FullType::Scalar(t) => (std::cmp::max(t.size() as usize, 1), std::cmp::max(t.size() as usize, 1)),
        FullType::Pointer(_) => (8, 8),
        FullType::Array { elem, size } => {
            let (elem_size, elem_align) = member_size_align(elem, struct_defs);
            let total = elem_size * size;
            // Inside structs, array alignment is just the element alignment
            (total, elem_align)
        }
        FullType::Struct(tag) => {
            if let Some(def) = struct_defs.get(tag) {
                (def.size, def.alignment)
            } else {
                panic!("Undefined struct: {}", tag);
            }
        }
    }
}

// ============================================================
// Static Initializer (for arrays and scalars)
// ============================================================

#[derive(Debug, Clone)]
pub enum StaticInit {
    IntInit(i32),
    LongInit(i64),
    UIntInit(u32),
    ULongInit(u64),
    CharInit(i8),
    UCharInit(u8),
    DoubleInit(f64),
    ZeroInit(usize), // zero-fill N bytes
    StringInit(String, bool), // (string_content, null_terminated) → .asciz or .ascii
    PointerInit(String), // label name → .quad label_name
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
    UIntLiteral(i64),
    ULongLiteral(i64),
    DoubleLiteral(f64),
    CharLiteral(i64),
    StringLiteral(String),
    // Keywords
    KWChar,
    KWSizeOf,
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
    KWRegister,
    KWBool,
    KWRestrict,
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
}

#[derive(Debug, Clone)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitwiseAnd,
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
pub enum Exp {
    Constant(i64),
    LongConstant(i64),
    UIntConstant(i64),
    ULongConstant(i64),
    DoubleConstant(f64),
    StringLiteral(String),
    Var(String),
    Cast(CType, Option<FullType>, Box<Exp>),
    Unary(UnaryOp, Box<Exp>),
    Binary(BinaryOp, Box<Exp>, Box<Exp>),
    Assign(Box<Exp>, Box<Exp>),
    CompoundAssign(BinaryOp, Box<Exp>, Box<Exp>),
    Conditional(Box<Exp>, Box<Exp>, Box<Exp>),
    FunctionCall(String, Vec<Exp>),
    Subscript(Box<Exp>, Box<Exp>), // arr[index]
    ArrayInit(Vec<Exp>),           // {1, 2, 3} or {{1,2}, {3,4}}
    SizeOf(Box<Exp>),              // sizeof expr
    SizeOfType(CType, FullType),   // sizeof(type)
    Dot(Box<Exp>, String),         // expr.member
    Arrow(Box<Exp>, String),       // expr->member
}

#[derive(Debug)]
pub enum ForInit {
    Declaration(VarDeclaration),
    Expression(Option<Exp>),
}

#[derive(Debug)]
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
        init: ForInit,
        condition: Option<Exp>,
        post: Option<Exp>,
        body: Box<Statement>,
        label: String,
    },
    Break(String),
    Continue(String),
    Goto(String),
    Label(String, Box<Statement>),
    Switch {
        control: Exp,
        body: Box<Statement>,
        label: String,
        cases: Vec<SwitchCase>,
    },
    Case {
        value: Exp,
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
    pub label: String,
}

pub type Block = Vec<BlockItem>;

#[derive(Debug)]
pub enum BlockItem {
    Declaration(Declaration),
    Statement(Statement),
}

#[derive(Debug, Clone, PartialEq)]
pub enum StorageClass {
    Static,
    Extern,
    Typedef,
}

#[derive(Debug)]
pub struct VarDeclaration {
    pub name: String,
    pub var_type: CType,
    /// For pointer variables: (base_type, pointer_depth)
    pub ptr_info: Option<(CType, usize)>,
    /// For array variables: (element_type, dimensions) e.g., int a[2][3] → (Int, [2,3])
    pub array_dims: Option<Vec<usize>>,
    /// Full derived type from declarator (includes pointer-to-array info)
    pub decl_full_type: Option<FullType>,
    pub init: Option<Exp>,
    pub storage_class: Option<StorageClass>,
}

#[derive(Debug)]
pub struct FunctionDeclaration {
    pub name: String,
    pub return_type: CType,
    pub return_ptr_info: Option<(CType, usize)>,
    pub return_full_type: Option<FullType>,
    /// Params: (name, type, optional ptr_info)
    pub params: Vec<(String, CType, Option<(CType, usize)>)>,
    /// Full types for each parameter (for proper multi-dim array tracking)
    pub param_full_types: Vec<FullType>,
    pub body: Option<Block>,
    pub storage_class: Option<StorageClass>,
}

#[derive(Debug, Clone)]
pub struct MemberDeclaration {
    pub name: String,
    pub member_type: CType,
    pub member_full_type: FullType,
}

#[derive(Debug)]
pub struct StructDeclaration {
    pub tag: String,
    pub members: Vec<MemberDeclaration>, // empty = incomplete type
    pub is_union: bool,
}

#[derive(Debug)]
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
    JumpIfZero(TackyVal, String),
    JumpIfNotZero(TackyVal, String),
    Label(String),
    FunCall {
        name: String,
        args: Vec<TackyVal>,
        dst: TackyVal,
        /// Indices of args that must be passed on the stack (MEMORY-class struct eightbytes)
        stack_arg_indices: std::collections::HashSet<usize>,
        /// Groups of consecutive args that form struct eightbytes (start_idx, count, is_sse_vec)
        struct_arg_groups: Vec<(usize, usize, Vec<bool>)>,
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
    DoubleToInt {
        src: TackyVal,
        dst: TackyVal,
    },
    UIntToDouble {
        src: TackyVal,
        dst: TackyVal,
    },
    DoubleToUInt {
        src: TackyVal,
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
}

#[derive(Debug)]
pub struct TackyProgram {
    pub top_level: Vec<TackyTopLevel>,
    pub global_vars: std::collections::HashSet<String>,
    pub symbol_types: std::collections::HashMap<String, CType>,
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
    Byte,     // 1-byte char
    Longword, // 32-bit int
    Quadword, // 64-bit long
    Double,   // 64-bit float (XMM)
}

impl From<CType> for AsmType {
    fn from(t: CType) -> Self {
        match t {
            CType::Char | CType::SChar | CType::UChar => AsmType::Byte,
            CType::Int | CType::UInt => AsmType::Longword,
            CType::Long | CType::ULong | CType::Pointer => AsmType::Quadword,
            CType::Double => AsmType::Double,
            CType::Void => AsmType::Longword,
            CType::Struct => AsmType::Longword, // struct size tracked separately
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum XmmReg {
    XMM0, XMM1, XMM2, XMM3, XMM4, XMM5, XMM6, XMM7,
    XMM8, XMM9, XMM10, XMM11, XMM12, XMM13, XMM14, XMM15,
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
    Data(String),
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
    Sub,
    Mul,
    DivDouble, // divsd (double division only)
    And,
    Or,
    Xor,
    Sal,
    Sar,
    Shr,
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
    Movsx(AsmType, AsmType, AsmOperand, AsmOperand),  // (src_type, dst_type, src, dst) sign-extend
    MovZeroExtend(AsmType, AsmType, AsmOperand, AsmOperand), // (src_type, dst_type, src, dst) zero-extend
    Unary(AsmType, AsmUnaryOp, AsmOperand),
    Binary(AsmType, AsmBinaryOp, AsmOperand, AsmOperand),
    Idiv(AsmType, AsmOperand),
    Div(AsmType, AsmOperand),  // unsigned division
    Cdq(AsmType),              // Longword=cdq, Quadword=cqo
    Cmp(AsmType, AsmOperand, AsmOperand),
    Jmp(String),
    JmpCC(CondCode, String),
    SetCC(CondCode, AsmOperand),
    Label(String),
    Push(AsmOperand),
    Call(String, usize, usize), // name, int_reg_args, sse_reg_args
    Pop(Reg),
    Cvtsi2sd(AsmType, AsmOperand, AsmOperand), // int/long → double
    Cvttsd2si(AsmType, AsmOperand, AsmOperand), // double → int/long (truncate)
    Lea(AsmOperand, AsmOperand),               // leaq src, dst
    /// Load from memory pointed to by a register: mov (reg), dst
    LoadIndirect(AsmType, Reg, AsmOperand),
    /// Store to memory pointed to by a register: mov src, (reg)
    StoreIndirect(AsmType, AsmOperand, Reg),
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
}

#[derive(Debug)]
pub struct AsmProgram {
    pub top_level: Vec<AsmTopLevel>,
}
