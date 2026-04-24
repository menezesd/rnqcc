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
    Executable,
}

// ============================================================
// C Types
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CType {
    Int,
    Long,
    UInt,
    ULong,
    Double,
    /// Pointer to some type. We don't track the pointee type at the assembly level —
    /// all pointers are 8 bytes. The pointee type is only needed for type checking
    /// which we handle during parsing/TACKY generation.
    Pointer,
    Void,
}

impl CType {
    pub fn size(self) -> i32 {
        match self {
            CType::Int | CType::UInt => 4,
            CType::Long | CType::ULong | CType::Double | CType::Pointer => 8,
            CType::Void => 0,
        }
    }

    pub fn is_signed(self) -> bool {
        matches!(self, CType::Int | CType::Long)
    }

    pub fn is_double(self) -> bool {
        self == CType::Double
    }

    pub fn is_pointer(self) -> bool {
        self == CType::Pointer
    }

    /// Usual arithmetic conversions (C standard 6.3.1.8)
    pub fn common(a: CType, b: CType) -> CType {
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
}

impl FullType {
    /// Convert to CType for codegen (arrays become ByteArray info, pointers become Pointer)
    pub fn to_ctype(&self) -> CType {
        match self {
            FullType::Scalar(t) => *t,
            FullType::Pointer(_) => CType::Pointer,
            FullType::Array { .. } => CType::Pointer, // arrays decay to pointers in most contexts
        }
    }

    /// Total byte size of this type
    pub fn byte_size(&self) -> usize {
        match self {
            FullType::Scalar(t) => t.size() as usize,
            FullType::Pointer(_) => 8,
            FullType::Array { elem, size } => elem.byte_size() * size,
        }
    }

    /// Alignment requirement
    pub fn alignment(&self) -> usize {
        match self {
            FullType::Scalar(t) => t.size() as usize,
            FullType::Pointer(_) => 8,
            FullType::Array { elem, .. } => {
                let ea = elem.alignment();
                if self.byte_size() >= 16 { std::cmp::max(ea, 16) } else { ea }
            }
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

// ============================================================
// Static Initializer (for arrays and scalars)
// ============================================================

#[derive(Debug, Clone)]
pub enum StaticInit {
    IntInit(i32),
    LongInit(i64),
    UIntInit(u32),
    ULongInit(u64),
    DoubleInit(f64),
    ZeroInit(usize), // zero-fill N bytes
    PointerInit(i64), // pointer value (address as i64)
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
    // Keywords
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
    Var(String),
    Cast(CType, Box<Exp>),
    Unary(UnaryOp, Box<Exp>),
    Binary(BinaryOp, Box<Exp>, Box<Exp>),
    Assign(Box<Exp>, Box<Exp>),
    CompoundAssign(BinaryOp, Box<Exp>, Box<Exp>),
    Conditional(Box<Exp>, Box<Exp>, Box<Exp>),
    FunctionCall(String, Vec<Exp>),
    Subscript(Box<Exp>, Box<Exp>), // arr[index]
    ArrayInit(Vec<Exp>),           // {1, 2, 3} or {{1,2}, {3,4}}
}

#[derive(Debug)]
pub enum ForInit {
    Declaration(VarDeclaration),
    Expression(Option<Exp>),
}

#[derive(Debug)]
pub enum Statement {
    Return(Exp),
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

#[derive(Debug)]
pub enum Declaration {
    FunDecl(FunctionDeclaration),
    VarDecl(VarDeclaration),
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
    DoubleConstant(f64),
    Var(String),
}

#[derive(Debug, Clone)]
pub enum TackyUnaryOp {
    Negate,
    Complement,
    LogicalNot,
}

#[derive(Debug, Clone)]
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

#[derive(Debug)]
pub enum TackyInstr {
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
    /// Add pointer + index * scale → dst
    AddPtr {
        ptr: TackyVal,
        index: TackyVal,
        scale: i64,
        dst: TackyVal,
    },
}

#[derive(Debug)]
pub struct TackyFunction {
    pub name: String,
    pub params: Vec<String>,
    pub global: bool,
    pub body: Vec<TackyInstr>,
}

#[derive(Debug)]
pub struct TackyStaticVar {
    pub name: String,
    pub global: bool,
    pub alignment: usize,
    pub init_values: Vec<StaticInit>,
}

#[derive(Debug)]
pub enum TackyTopLevel {
    Function(TackyFunction),
    StaticVar(TackyStaticVar),
}

#[derive(Debug)]
pub struct TackyProgram {
    pub top_level: Vec<TackyTopLevel>,
    pub global_vars: std::collections::HashSet<String>,
    pub symbol_types: std::collections::HashMap<String, CType>,
    /// Array storage sizes: maps "__array_storage_X" → total bytes
    pub array_sizes: std::collections::HashMap<String, usize>,
}

// ============================================================
// Assembly IR
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AsmType {
    Longword, // 32-bit int
    Quadword, // 64-bit long
    Double,   // 64-bit float (XMM)
}

impl From<CType> for AsmType {
    fn from(t: CType) -> Self {
        match t {
            CType::Int | CType::UInt => AsmType::Longword,
            CType::Long | CType::ULong | CType::Pointer => AsmType::Quadword,
            CType::Double => AsmType::Double,
            CType::Void => AsmType::Longword,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum XmmReg {
    XMM0, XMM1, XMM2, XMM3, XMM4, XMM5, XMM6, XMM7,
    XMM14, XMM15,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reg {
    AX,
    CX,
    DX,
    DI,
    SI,
    R8,
    R9,
    R10,
    R11,
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

#[derive(Debug)]
pub enum AsmInstr {
    Mov(AsmType, AsmOperand, AsmOperand),
    Movsx(AsmOperand, AsmOperand),  // movslq: sign-extend 32→64
    MovZeroExtend(AsmOperand, AsmOperand), // movl (32→64 zero-extend via movl)
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
    Call(String),
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
pub enum AsmTopLevel {
    Function(AsmFunction),
    StaticVar(AsmStaticVar),
}

#[derive(Debug)]
pub struct AsmProgram {
    pub top_level: Vec<AsmTopLevel>,
}
