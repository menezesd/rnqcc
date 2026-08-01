// ============================================================
// Shared backend types (AsmType, Reg, XmmReg, etc.)
// ============================================================
// These types are shared between x86_64 and AArch64 backends.

/// Split a signed 128-bit integer into low and high 64-bit halves.
pub fn i128_parts_signed(value: i128) -> (i64, i64) {
    (value as i64, (value >> 64) as i64)
}

/// Split an unsigned 128-bit integer into low and high 64-bit halves.
pub fn i128_parts_unsigned(value: u128) -> (i64, i64) {
    (value as u64 as i64, (value >> 64) as u64 as i64)
}

/// Variables that may be accessed indirectly and so cannot live purely in a
/// register: every static/global var plus any local whose address is taken.
/// ABI-independent, shared by both backends.
pub fn compute_aliased(
    body: &[crate::types::TackyInstr],
    static_vars: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    use crate::types::{TackyInstr, TackyVal};
    let mut aliased = static_vars.clone();
    for instr in body {
        if let TackyInstr::GetAddress {
            src: TackyVal::Var(name),
            ..
        } = instr
        {
            aliased.insert(name.clone());
        }
    }
    aliased
}

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

impl From<crate::types::CType> for AsmType {
    fn from(t: crate::types::CType) -> Self {
        use crate::types::CType;
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
    Stack(i64),
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
    Imul,
    Div,
    Idiv,
    SDiv,
    UDiv,
    DivDouble,
    And,
    Nand,
    Or,
    Xor,
    Sal,
    Sar,
    Shr,
    Cmp,
    Test,
    SetCC,
}

#[derive(Debug, Clone)]
pub enum AsmX87BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Cmp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondCode {
    E,
    NE,
    L,
    LE,
    G,
    GE,
    // Unsigned
    A,
    AE,
    B,
    BE,
    P,
    NP,
    S,
    NS,
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
    Call(String, usize, usize, bool, bool), // name, int_reg_args, sse_reg_args, indirect, local
    Pop(Reg),
    Cvtsi2sd(AsmType, AsmOperand, AsmOperand), // int/long -> double
    Cvtsi2ss(AsmType, AsmOperand, AsmOperand), // int/long -> float
    Cvttsd2si(AsmType, AsmOperand, AsmOperand), // double -> int/long (truncate)
    Cvttss2si(AsmType, AsmOperand, AsmOperand), // float -> int/long (truncate)
    Lea(AsmOperand, AsmOperand),               // Load effective address
    And(AsmType, AsmOperand, AsmOperand),      // bitwise and
    Or(AsmType, AsmOperand, AsmOperand),       // bitwise or
    Xor(AsmType, AsmOperand, AsmOperand),      // bitwise xor
    Test(AsmType, AsmOperand, AsmOperand),     // bitwise test
    Shl(AsmType, AsmOperand, AsmOperand),      // shift left
    Shr(AsmType, AsmOperand, AsmOperand),      // shift right
    Sar(AsmType, AsmOperand, AsmOperand),      // arithmetic shift right
    Ror(AsmType, AsmOperand, AsmOperand),      // rotate right
    Rol(AsmType, AsmOperand, AsmOperand),      // rotate left
    Cvtss2sd(AsmOperand, AsmOperand),          // float -> double
    Cvtsd2ss(AsmOperand, AsmOperand),          // double -> float
    X87Binary(AsmX87BinaryOp),
    Fld(AsmType, AsmOperand),     // Load x87 stack
    Fstp(AsmType, AsmOperand),    // Store and pop x87 stack
    Fisttp(AsmType, AsmOperand),  // Store x87 as integer and pop
    Fxch,                         // Exchange x87 top two registers
    FstpQ,                        // Store 80-bit and pop (x87 long double)
    FldQ(AsmOperand),             // Load 80-bit (x87 long double)
    X87Push(AsmType, AsmOperand), // Push to x87
    X87Pop(AsmType, AsmOperand),  // Pop from x87
    Unreachable,
    Ret,
    AllocateStack(i64),
    DeallocateStack(i64),
    LoadIndirect(AsmType, Reg, AsmOperand),
    StoreIndirect(AsmType, AsmOperand, Reg),
    CopyToStackArg {
        src_ptr: AsmOperand,
        dst_offset: i32,
        size: usize,
    },
    CopyFromStackArg {
        src_offset: i32,
        dst: AsmOperand,
        size: usize,
    },
    AArch64AddPtr(AsmOperand, AsmOperand, i64, AsmOperand),
    AArch64Extr(AsmOperand, AsmOperand, u8, AsmOperand),
    AArch64Umulh(AsmOperand, AsmOperand, AsmOperand),
    AArch64LoadAdjusted(AsmType, AsmOperand, Reg, i32),
    AArch64StoreOutgoingArg(AsmType, AsmOperand, i32, i32),
    AArch64Rem(AsmType, bool, AsmOperand, AsmOperand, AsmOperand),
    AArch64SaveLink(i32),
    AArch64RestoreLink(i32),
    AArch64AllocateLargeStack(i64),
    AArch64DeallocateLargeStack(i64),
    AArch64StoreLargeLocalBase {
        base_offset: i64,
        dst_offset: i32,
    },
    AArch64UIntToDouble(AsmType, AsmOperand, AsmOperand),
    AArch64UIntToFloat(AsmType, AsmOperand, AsmOperand),
    AArch64DoubleToUInt(AsmType, AsmOperand, AsmOperand),
    AArch64FloatToUInt(AsmType, AsmOperand, AsmOperand),
    AArch64FloatToDouble(AsmOperand, AsmOperand),
    AArch64DoubleToFloat(AsmOperand, AsmOperand),
    X86SetVarargsXmmCount(usize),
    AtomicFence,
    AtomicRmw(AsmType, AsmBinaryOp, bool, AsmOperand),
    AtomicExchange(AsmType, AsmOperand),
    AtomicCompareExchange(AsmType, AsmOperand),
    AtomicCompareSwap(AsmType, bool, AsmOperand),
    X87Load(AsmType, AsmOperand),
    X87LoadIndirect(AsmType, Reg),
    X87Store(AsmOperand),
    X87StoreFloat(AsmType, AsmOperand),
    X87StoreIndirect(Reg),
    X87StoreInt(AsmType, AsmOperand),
    X87UnaryNeg,
    X87Compare,
}
