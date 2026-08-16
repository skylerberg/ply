/// An instruction format
///
/// Every opcode has a corresponding instruction format
/// which is represented by both the `InstructionFormat`
/// and the `InstructionData` enums.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum InstructionFormat {
    /// AtomicCas(imms=(flags: ir::MemFlags), vals=3, blocks=0, raw_blocks=0)
    AtomicCas, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// AtomicRmw(imms=(flags: ir::MemFlags, op: ir::AtomicRmwOp), vals=2, blocks=0, raw_blocks=0)
    AtomicRmw, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// Binary(imms=(), vals=2, blocks=0, raw_blocks=0)
    Binary, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// BinaryImm8(imms=(imm: ir::immediates::Uimm8), vals=1, blocks=0, raw_blocks=0)
    BinaryImm8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// BranchTable(imms=(table: ir::JumpTable), vals=1, blocks=0, raw_blocks=0)
    BranchTable, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// Brif(imms=(), vals=1, blocks=2, raw_blocks=0)
    Brif, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// Call(imms=(func_ref: ir::FuncRef), vals=0, blocks=0, raw_blocks=0)
    Call, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// CallIndirect(imms=(sig_ref: ir::SigRef), vals=1, blocks=0, raw_blocks=0)
    CallIndirect, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// CondTrap(imms=(code: ir::TrapCode), vals=1, blocks=0, raw_blocks=0)
    CondTrap, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// DynamicStackAddr(imms=(dynamic_stack_slot: ir::DynamicStackSlot), vals=0, blocks=0, raw_blocks=0)
    DynamicStackAddr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// ExceptionHandlerAddress(imms=(imm: ir::immediates::Imm64), vals=0, blocks=0, raw_blocks=1)
    ExceptionHandlerAddress, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// FloatCompare(imms=(cond: ir::condcodes::FloatCC), vals=2, blocks=0, raw_blocks=0)
    FloatCompare, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// FuncAddr(imms=(func_ref: ir::FuncRef), vals=0, blocks=0, raw_blocks=0)
    FuncAddr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// IntAddTrap(imms=(code: ir::TrapCode), vals=2, blocks=0, raw_blocks=0)
    IntAddTrap, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// IntCompare(imms=(cond: ir::condcodes::IntCC), vals=2, blocks=0, raw_blocks=0)
    IntCompare, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// Jump(imms=(), vals=0, blocks=1, raw_blocks=0)
    Jump, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// Load(imms=(flags: ir::MemFlags, offset: ir::immediates::Offset32), vals=1, blocks=0, raw_blocks=0)
    Load, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// LoadNoOffset(imms=(flags: ir::MemFlags), vals=1, blocks=0, raw_blocks=0)
    LoadNoOffset, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// MultiAry(imms=(), vals=0, blocks=0, raw_blocks=0)
    MultiAry, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// NullAry(imms=(), vals=0, blocks=0, raw_blocks=0)
    NullAry, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// Shuffle(imms=(imm: ir::Immediate), vals=2, blocks=0, raw_blocks=0)
    Shuffle, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// StackAddr(imms=(stack_slot: ir::StackSlot, offset: ir::immediates::Offset32), vals=0, blocks=0, raw_blocks=0)
    StackAddr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// Store(imms=(flags: ir::MemFlags, offset: ir::immediates::Offset32), vals=2, blocks=0, raw_blocks=0)
    Store, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// StoreNoOffset(imms=(flags: ir::MemFlags), vals=2, blocks=0, raw_blocks=0)
    StoreNoOffset, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// Ternary(imms=(), vals=3, blocks=0, raw_blocks=0)
    Ternary, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// TernaryImm8(imms=(imm: ir::immediates::Uimm8), vals=2, blocks=0, raw_blocks=0)
    TernaryImm8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// Trap(imms=(code: ir::TrapCode), vals=0, blocks=0, raw_blocks=0)
    Trap, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// TryCall(imms=(func_ref: ir::FuncRef, exception: ir::ExceptionTable), vals=0, blocks=0, raw_blocks=0)
    TryCall, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// TryCallIndirect(imms=(exception: ir::ExceptionTable), vals=1, blocks=0, raw_blocks=0)
    TryCallIndirect, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// Unary(imms=(), vals=1, blocks=0, raw_blocks=0)
    Unary, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// UnaryConst(imms=(constant_handle: ir::Constant), vals=0, blocks=0, raw_blocks=0)
    UnaryConst, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// UnaryGlobalValue(imms=(global_value: ir::GlobalValue), vals=0, blocks=0, raw_blocks=0)
    UnaryGlobalValue, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// UnaryIeee16(imms=(imm: ir::immediates::Ieee16), vals=0, blocks=0, raw_blocks=0)
    UnaryIeee16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// UnaryIeee32(imms=(imm: ir::immediates::Ieee32), vals=0, blocks=0, raw_blocks=0)
    UnaryIeee32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// UnaryIeee64(imms=(imm: ir::immediates::Ieee64), vals=0, blocks=0, raw_blocks=0)
    UnaryIeee64, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
    /// UnaryImm(imms=(imm: ir::immediates::Imm64), vals=0, blocks=0, raw_blocks=0)
    UnaryImm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:32
}

impl<'a> From<&'a InstructionData> for InstructionFormat {
    fn from(inst: &'a InstructionData) -> Self {
        match *inst {
            InstructionData::AtomicCas { .. } => {
                Self::AtomicCas
            }
            InstructionData::AtomicRmw { .. } => {
                Self::AtomicRmw
            }
            InstructionData::Binary { .. } => {
                Self::Binary
            }
            InstructionData::BinaryImm8 { .. } => {
                Self::BinaryImm8
            }
            InstructionData::BranchTable { .. } => {
                Self::BranchTable
            }
            InstructionData::Brif { .. } => {
                Self::Brif
            }
            InstructionData::Call { .. } => {
                Self::Call
            }
            InstructionData::CallIndirect { .. } => {
                Self::CallIndirect
            }
            InstructionData::CondTrap { .. } => {
                Self::CondTrap
            }
            InstructionData::DynamicStackAddr { .. } => {
                Self::DynamicStackAddr
            }
            InstructionData::ExceptionHandlerAddress { .. } => {
                Self::ExceptionHandlerAddress
            }
            InstructionData::FloatCompare { .. } => {
                Self::FloatCompare
            }
            InstructionData::FuncAddr { .. } => {
                Self::FuncAddr
            }
            InstructionData::IntAddTrap { .. } => {
                Self::IntAddTrap
            }
            InstructionData::IntCompare { .. } => {
                Self::IntCompare
            }
            InstructionData::Jump { .. } => {
                Self::Jump
            }
            InstructionData::Load { .. } => {
                Self::Load
            }
            InstructionData::LoadNoOffset { .. } => {
                Self::LoadNoOffset
            }
            InstructionData::MultiAry { .. } => {
                Self::MultiAry
            }
            InstructionData::NullAry { .. } => {
                Self::NullAry
            }
            InstructionData::Shuffle { .. } => {
                Self::Shuffle
            }
            InstructionData::StackAddr { .. } => {
                Self::StackAddr
            }
            InstructionData::Store { .. } => {
                Self::Store
            }
            InstructionData::StoreNoOffset { .. } => {
                Self::StoreNoOffset
            }
            InstructionData::Ternary { .. } => {
                Self::Ternary
            }
            InstructionData::TernaryImm8 { .. } => {
                Self::TernaryImm8
            }
            InstructionData::Trap { .. } => {
                Self::Trap
            }
            InstructionData::TryCall { .. } => {
                Self::TryCall
            }
            InstructionData::TryCallIndirect { .. } => {
                Self::TryCallIndirect
            }
            InstructionData::Unary { .. } => {
                Self::Unary
            }
            InstructionData::UnaryConst { .. } => {
                Self::UnaryConst
            }
            InstructionData::UnaryGlobalValue { .. } => {
                Self::UnaryGlobalValue
            }
            InstructionData::UnaryIeee16 { .. } => {
                Self::UnaryIeee16
            }
            InstructionData::UnaryIeee32 { .. } => {
                Self::UnaryIeee32
            }
            InstructionData::UnaryIeee64 { .. } => {
                Self::UnaryIeee64
            }
            InstructionData::UnaryImm { .. } => {
                Self::UnaryImm
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "enable-serde", derive(Serialize, Deserialize))]
#[allow(missing_docs, reason = "generated code")]
pub enum InstructionData {
    AtomicCas {
        opcode: Opcode,
        args: [Value; 3], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:76
        flags: ir::MemFlags, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    AtomicRmw {
        opcode: Opcode,
        args: [Value; 2], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:76
        flags: ir::MemFlags, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
        op: ir::AtomicRmwOp, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    Binary {
        opcode: Opcode,
        args: [Value; 2], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:76
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    BinaryImm8 {
        opcode: Opcode,
        arg: Value,
        imm: ir::immediates::Uimm8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    BranchTable {
        opcode: Opcode,
        arg: Value,
        table: ir::JumpTable, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    Brif {
        opcode: Opcode,
        arg: Value,
        blocks: [ir::BlockCall; 2], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:82
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    Call {
        opcode: Opcode,
        args: ValueList,
        func_ref: ir::FuncRef, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    CallIndirect {
        opcode: Opcode,
        args: ValueList,
        sig_ref: ir::SigRef, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    CondTrap {
        opcode: Opcode,
        arg: Value,
        code: ir::TrapCode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    DynamicStackAddr {
        opcode: Opcode,
        dynamic_stack_slot: ir::DynamicStackSlot, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    ExceptionHandlerAddress {
        opcode: Opcode,
        block: ir::Block,
        imm: ir::immediates::Imm64, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    FloatCompare {
        opcode: Opcode,
        args: [Value; 2], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:76
        cond: ir::condcodes::FloatCC, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    FuncAddr {
        opcode: Opcode,
        func_ref: ir::FuncRef, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    IntAddTrap {
        opcode: Opcode,
        args: [Value; 2], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:76
        code: ir::TrapCode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    IntCompare {
        opcode: Opcode,
        args: [Value; 2], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:76
        cond: ir::condcodes::IntCC, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    Jump {
        opcode: Opcode,
        destination: ir::BlockCall,
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    Load {
        opcode: Opcode,
        arg: Value,
        flags: ir::MemFlags, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
        offset: ir::immediates::Offset32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    LoadNoOffset {
        opcode: Opcode,
        arg: Value,
        flags: ir::MemFlags, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    MultiAry {
        opcode: Opcode,
        args: ValueList,
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    NullAry {
        opcode: Opcode,
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    Shuffle {
        opcode: Opcode,
        args: [Value; 2], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:76
        imm: ir::Immediate, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    StackAddr {
        opcode: Opcode,
        stack_slot: ir::StackSlot, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
        offset: ir::immediates::Offset32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    Store {
        opcode: Opcode,
        args: [Value; 2], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:76
        flags: ir::MemFlags, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
        offset: ir::immediates::Offset32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    StoreNoOffset {
        opcode: Opcode,
        args: [Value; 2], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:76
        flags: ir::MemFlags, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    Ternary {
        opcode: Opcode,
        args: [Value; 3], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:76
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    TernaryImm8 {
        opcode: Opcode,
        args: [Value; 2], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:76
        imm: ir::immediates::Uimm8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    Trap {
        opcode: Opcode,
        code: ir::TrapCode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    TryCall {
        opcode: Opcode,
        args: ValueList,
        func_ref: ir::FuncRef, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
        exception: ir::ExceptionTable, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    TryCallIndirect {
        opcode: Opcode,
        args: ValueList,
        exception: ir::ExceptionTable, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    Unary {
        opcode: Opcode,
        arg: Value,
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    UnaryConst {
        opcode: Opcode,
        constant_handle: ir::Constant, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    UnaryGlobalValue {
        opcode: Opcode,
        global_value: ir::GlobalValue, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    UnaryIeee16 {
        opcode: Opcode,
        imm: ir::immediates::Ieee16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    UnaryIeee32 {
        opcode: Opcode,
        imm: ir::immediates::Ieee32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    UnaryIeee64 {
        opcode: Opcode,
        imm: ir::immediates::Ieee64, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
    UnaryImm {
        opcode: Opcode,
        imm: ir::immediates::Imm64, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:97
    }
    , // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:100
}

impl InstructionData {
    /// Get the opcode of this instruction.
    pub fn opcode(&self) -> Opcode {
        match *self {
            Self::AtomicCas { opcode, .. } |
            Self::AtomicRmw { opcode, .. } |
            Self::Binary { opcode, .. } |
            Self::BinaryImm8 { opcode, .. } |
            Self::BranchTable { opcode, .. } |
            Self::Brif { opcode, .. } |
            Self::Call { opcode, .. } |
            Self::CallIndirect { opcode, .. } |
            Self::CondTrap { opcode, .. } |
            Self::DynamicStackAddr { opcode, .. } |
            Self::ExceptionHandlerAddress { opcode, .. } |
            Self::FloatCompare { opcode, .. } |
            Self::FuncAddr { opcode, .. } |
            Self::IntAddTrap { opcode, .. } |
            Self::IntCompare { opcode, .. } |
            Self::Jump { opcode, .. } |
            Self::Load { opcode, .. } |
            Self::LoadNoOffset { opcode, .. } |
            Self::MultiAry { opcode, .. } |
            Self::NullAry { opcode, .. } |
            Self::Shuffle { opcode, .. } |
            Self::StackAddr { opcode, .. } |
            Self::Store { opcode, .. } |
            Self::StoreNoOffset { opcode, .. } |
            Self::Ternary { opcode, .. } |
            Self::TernaryImm8 { opcode, .. } |
            Self::Trap { opcode, .. } |
            Self::TryCall { opcode, .. } |
            Self::TryCallIndirect { opcode, .. } |
            Self::Unary { opcode, .. } |
            Self::UnaryConst { opcode, .. } |
            Self::UnaryGlobalValue { opcode, .. } |
            Self::UnaryIeee16 { opcode, .. } |
            Self::UnaryIeee32 { opcode, .. } |
            Self::UnaryIeee64 { opcode, .. } |
            Self::UnaryImm { opcode, .. } => {
                opcode
            }
        }
    }

    /// Get the controlling type variable operand.
    pub fn typevar_operand(&self, pool: &ir::ValueListPool) -> Option<Value> {
        match *self {
            Self::Call { .. } |
            Self::DynamicStackAddr { .. } |
            Self::ExceptionHandlerAddress { .. } |
            Self::FuncAddr { .. } |
            Self::Jump { .. } |
            Self::MultiAry { .. } |
            Self::NullAry { .. } |
            Self::StackAddr { .. } |
            Self::Trap { .. } |
            Self::TryCall { .. } |
            Self::UnaryConst { .. } |
            Self::UnaryGlobalValue { .. } |
            Self::UnaryIeee16 { .. } |
            Self::UnaryIeee32 { .. } |
            Self::UnaryIeee64 { .. } |
            Self::UnaryImm { .. } => {
                None
            }
            Self::BinaryImm8 { arg, .. } |
            Self::BranchTable { arg, .. } |
            Self::Brif { arg, .. } |
            Self::CondTrap { arg, .. } |
            Self::Load { arg, .. } |
            Self::LoadNoOffset { arg, .. } |
            Self::Unary { arg, .. } => {
                Some(arg)
            }
            Self::AtomicRmw { args: ref args_arity2, .. } |
            Self::Binary { args: ref args_arity2, .. } |
            Self::FloatCompare { args: ref args_arity2, .. } |
            Self::IntAddTrap { args: ref args_arity2, .. } |
            Self::IntCompare { args: ref args_arity2, .. } |
            Self::Shuffle { args: ref args_arity2, .. } |
            Self::Store { args: ref args_arity2, .. } |
            Self::StoreNoOffset { args: ref args_arity2, .. } |
            Self::TernaryImm8 { args: ref args_arity2, .. } => {
                Some(args_arity2[0])
            }
            Self::Ternary { args: ref args_arity3, .. } => {
                Some(args_arity3[1])
            }
            Self::AtomicCas { args: ref args_arity3, .. } => {
                Some(args_arity3[2])
            }
            Self::CallIndirect { ref args, .. } |
            Self::TryCallIndirect { ref args, .. } => {
                args.get(0, pool)
            }
        }
    }

    /// Get the value arguments to this instruction.
    pub fn arguments<'a>(&'a self, pool: &'a ir::ValueListPool) -> &'a [Value] {
        match *self {
            Self::DynamicStackAddr { .. } |
            Self::ExceptionHandlerAddress { .. } |
            Self::FuncAddr { .. } |
            Self::Jump { .. } |
            Self::NullAry { .. } |
            Self::StackAddr { .. } |
            Self::Trap { .. } |
            Self::UnaryConst { .. } |
            Self::UnaryGlobalValue { .. } |
            Self::UnaryIeee16 { .. } |
            Self::UnaryIeee32 { .. } |
            Self::UnaryIeee64 { .. } |
            Self::UnaryImm { .. } => {
                &[]
            }
            Self::AtomicRmw { args: ref args_arity2, .. } |
            Self::Binary { args: ref args_arity2, .. } |
            Self::FloatCompare { args: ref args_arity2, .. } |
            Self::IntAddTrap { args: ref args_arity2, .. } |
            Self::IntCompare { args: ref args_arity2, .. } |
            Self::Shuffle { args: ref args_arity2, .. } |
            Self::Store { args: ref args_arity2, .. } |
            Self::StoreNoOffset { args: ref args_arity2, .. } |
            Self::TernaryImm8 { args: ref args_arity2, .. } => {
                args_arity2
            }
            Self::AtomicCas { args: ref args_arity3, .. } |
            Self::Ternary { args: ref args_arity3, .. } => {
                args_arity3
            }
            Self::BinaryImm8 { ref arg, .. } |
            Self::BranchTable { ref arg, .. } |
            Self::Brif { ref arg, .. } |
            Self::CondTrap { ref arg, .. } |
            Self::Load { ref arg, .. } |
            Self::LoadNoOffset { ref arg, .. } |
            Self::Unary { ref arg, .. } => {
                core::slice::from_ref(arg)
            }
            Self::Call { ref args, .. } |
            Self::CallIndirect { ref args, .. } |
            Self::MultiAry { ref args, .. } |
            Self::TryCall { ref args, .. } |
            Self::TryCallIndirect { ref args, .. } => {
                args.as_slice(pool)
            }
        }
    }

    /// Get mutable references to the value arguments to this
    /// instruction.
    pub fn arguments_mut<'a>(&'a mut self, pool: &'a mut ir::ValueListPool) -> &'a mut [Value] {
        match *self {
            Self::DynamicStackAddr { .. } |
            Self::ExceptionHandlerAddress { .. } |
            Self::FuncAddr { .. } |
            Self::Jump { .. } |
            Self::NullAry { .. } |
            Self::StackAddr { .. } |
            Self::Trap { .. } |
            Self::UnaryConst { .. } |
            Self::UnaryGlobalValue { .. } |
            Self::UnaryIeee16 { .. } |
            Self::UnaryIeee32 { .. } |
            Self::UnaryIeee64 { .. } |
            Self::UnaryImm { .. } => {
                &mut []
            }
            Self::AtomicRmw { args: ref mut args_arity2, .. } |
            Self::Binary { args: ref mut args_arity2, .. } |
            Self::FloatCompare { args: ref mut args_arity2, .. } |
            Self::IntAddTrap { args: ref mut args_arity2, .. } |
            Self::IntCompare { args: ref mut args_arity2, .. } |
            Self::Shuffle { args: ref mut args_arity2, .. } |
            Self::Store { args: ref mut args_arity2, .. } |
            Self::StoreNoOffset { args: ref mut args_arity2, .. } |
            Self::TernaryImm8 { args: ref mut args_arity2, .. } => {
                args_arity2
            }
            Self::AtomicCas { args: ref mut args_arity3, .. } |
            Self::Ternary { args: ref mut args_arity3, .. } => {
                args_arity3
            }
            Self::BinaryImm8 { ref mut arg, .. } |
            Self::BranchTable { ref mut arg, .. } |
            Self::Brif { ref mut arg, .. } |
            Self::CondTrap { ref mut arg, .. } |
            Self::Load { ref mut arg, .. } |
            Self::LoadNoOffset { ref mut arg, .. } |
            Self::Unary { ref mut arg, .. } => {
                core::slice::from_mut(arg)
            }
            Self::Call { ref mut args, .. } |
            Self::CallIndirect { ref mut args, .. } |
            Self::MultiAry { ref mut args, .. } |
            Self::TryCall { ref mut args, .. } |
            Self::TryCallIndirect { ref mut args, .. } => {
                args.as_mut_slice(pool)
            }
        }
    }

    /// Compare two `InstructionData` for equality.
    ///
    /// This operation requires a reference to a `ValueListPool` to
    /// determine if the contents of any `ValueLists` are equal.
    ///
    /// This operation takes a closure that is allowed to map each
    /// argument value to some other value before the instructions
    /// are compared. This allows various forms of canonicalization.
    pub fn eq(&self, other: &Self, pool: &ir::ValueListPool) -> bool {
        if ::core::mem::discriminant(self) != ::core::mem::discriminant(other) {
            return false;
        }
        match (self, other) {
            (&Self::AtomicCas { opcode: ref opcode1, args: ref args1, flags: ref flags1 }, &Self::AtomicCas { opcode: ref opcode2, args: ref args2, flags: ref flags2 }) =>  {
                opcode1 == opcode2
                && flags1 == flags2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && args1.iter().zip(args2.iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::AtomicRmw { opcode: ref opcode1, args: ref args1, flags: ref flags1, op: ref op1 }, &Self::AtomicRmw { opcode: ref opcode2, args: ref args2, flags: ref flags2, op: ref op2 }) =>  {
                opcode1 == opcode2
                && flags1 == flags2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && op1 == op2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && args1.iter().zip(args2.iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::Binary { opcode: ref opcode1, args: ref args1 }, &Self::Binary { opcode: ref opcode2, args: ref args2 }) =>  {
                opcode1 == opcode2
                && args1.iter().zip(args2.iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::BinaryImm8 { opcode: ref opcode1, arg: ref arg1, imm: ref imm1 }, &Self::BinaryImm8 { opcode: ref opcode2, arg: ref arg2, imm: ref imm2 }) =>  {
                opcode1 == opcode2
                && imm1 == imm2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && arg1 == arg2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::BranchTable { opcode: ref opcode1, arg: ref arg1, table: ref table1 }, &Self::BranchTable { opcode: ref opcode2, arg: ref arg2, table: ref table2 }) =>  {
                opcode1 == opcode2
                && table1 == table2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && arg1 == arg2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::Brif { opcode: ref opcode1, arg: ref arg1, blocks: ref blocks1 }, &Self::Brif { opcode: ref opcode2, arg: ref arg2, blocks: ref blocks2 }) =>  {
                opcode1 == opcode2
                && arg1 == arg2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
                && blocks1.iter().zip(blocks2.iter()).all(|(a, b)| a.block(pool) == b.block(pool)) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:282
            }
            (&Self::Call { opcode: ref opcode1, args: ref args1, func_ref: ref func_ref1 }, &Self::Call { opcode: ref opcode2, args: ref args2, func_ref: ref func_ref2 }) =>  {
                opcode1 == opcode2
                && func_ref1 == func_ref2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && args1.as_slice(pool).iter().zip(args2.as_slice(pool).iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::CallIndirect { opcode: ref opcode1, args: ref args1, sig_ref: ref sig_ref1 }, &Self::CallIndirect { opcode: ref opcode2, args: ref args2, sig_ref: ref sig_ref2 }) =>  {
                opcode1 == opcode2
                && sig_ref1 == sig_ref2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && args1.as_slice(pool).iter().zip(args2.as_slice(pool).iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::CondTrap { opcode: ref opcode1, arg: ref arg1, code: ref code1 }, &Self::CondTrap { opcode: ref opcode2, arg: ref arg2, code: ref code2 }) =>  {
                opcode1 == opcode2
                && code1 == code2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && arg1 == arg2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::DynamicStackAddr { opcode: ref opcode1, dynamic_stack_slot: ref dynamic_stack_slot1 }, &Self::DynamicStackAddr { opcode: ref opcode2, dynamic_stack_slot: ref dynamic_stack_slot2 }) =>  {
                opcode1 == opcode2
                && dynamic_stack_slot1 == dynamic_stack_slot2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
            }
            (&Self::ExceptionHandlerAddress { opcode: ref opcode1, block: ref block1, imm: ref imm1 }, &Self::ExceptionHandlerAddress { opcode: ref opcode2, block: ref block2, imm: ref imm2 }) =>  {
                opcode1 == opcode2
                && imm1 == imm2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && block1 == block2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:285
            }
            (&Self::FloatCompare { opcode: ref opcode1, args: ref args1, cond: ref cond1 }, &Self::FloatCompare { opcode: ref opcode2, args: ref args2, cond: ref cond2 }) =>  {
                opcode1 == opcode2
                && cond1 == cond2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && args1.iter().zip(args2.iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::FuncAddr { opcode: ref opcode1, func_ref: ref func_ref1 }, &Self::FuncAddr { opcode: ref opcode2, func_ref: ref func_ref2 }) =>  {
                opcode1 == opcode2
                && func_ref1 == func_ref2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
            }
            (&Self::IntAddTrap { opcode: ref opcode1, args: ref args1, code: ref code1 }, &Self::IntAddTrap { opcode: ref opcode2, args: ref args2, code: ref code2 }) =>  {
                opcode1 == opcode2
                && code1 == code2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && args1.iter().zip(args2.iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::IntCompare { opcode: ref opcode1, args: ref args1, cond: ref cond1 }, &Self::IntCompare { opcode: ref opcode2, args: ref args2, cond: ref cond2 }) =>  {
                opcode1 == opcode2
                && cond1 == cond2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && args1.iter().zip(args2.iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::Jump { opcode: ref opcode1, destination: ref destination1 }, &Self::Jump { opcode: ref opcode2, destination: ref destination2 }) =>  {
                opcode1 == opcode2
                && destination1 == destination2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:282
            }
            (&Self::Load { opcode: ref opcode1, arg: ref arg1, flags: ref flags1, offset: ref offset1 }, &Self::Load { opcode: ref opcode2, arg: ref arg2, flags: ref flags2, offset: ref offset2 }) =>  {
                opcode1 == opcode2
                && flags1 == flags2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && offset1 == offset2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && arg1 == arg2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::LoadNoOffset { opcode: ref opcode1, arg: ref arg1, flags: ref flags1 }, &Self::LoadNoOffset { opcode: ref opcode2, arg: ref arg2, flags: ref flags2 }) =>  {
                opcode1 == opcode2
                && flags1 == flags2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && arg1 == arg2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::MultiAry { opcode: ref opcode1, args: ref args1 }, &Self::MultiAry { opcode: ref opcode2, args: ref args2 }) =>  {
                opcode1 == opcode2
                && args1.as_slice(pool).iter().zip(args2.as_slice(pool).iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::NullAry { opcode: ref opcode1 }, &Self::NullAry { opcode: ref opcode2 }) =>  {
                opcode1 == opcode2
            }
            (&Self::Shuffle { opcode: ref opcode1, args: ref args1, imm: ref imm1 }, &Self::Shuffle { opcode: ref opcode2, args: ref args2, imm: ref imm2 }) =>  {
                opcode1 == opcode2
                && imm1 == imm2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && args1.iter().zip(args2.iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::StackAddr { opcode: ref opcode1, stack_slot: ref stack_slot1, offset: ref offset1 }, &Self::StackAddr { opcode: ref opcode2, stack_slot: ref stack_slot2, offset: ref offset2 }) =>  {
                opcode1 == opcode2
                && stack_slot1 == stack_slot2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && offset1 == offset2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
            }
            (&Self::Store { opcode: ref opcode1, args: ref args1, flags: ref flags1, offset: ref offset1 }, &Self::Store { opcode: ref opcode2, args: ref args2, flags: ref flags2, offset: ref offset2 }) =>  {
                opcode1 == opcode2
                && flags1 == flags2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && offset1 == offset2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && args1.iter().zip(args2.iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::StoreNoOffset { opcode: ref opcode1, args: ref args1, flags: ref flags1 }, &Self::StoreNoOffset { opcode: ref opcode2, args: ref args2, flags: ref flags2 }) =>  {
                opcode1 == opcode2
                && flags1 == flags2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && args1.iter().zip(args2.iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::Ternary { opcode: ref opcode1, args: ref args1 }, &Self::Ternary { opcode: ref opcode2, args: ref args2 }) =>  {
                opcode1 == opcode2
                && args1.iter().zip(args2.iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::TernaryImm8 { opcode: ref opcode1, args: ref args1, imm: ref imm1 }, &Self::TernaryImm8 { opcode: ref opcode2, args: ref args2, imm: ref imm2 }) =>  {
                opcode1 == opcode2
                && imm1 == imm2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && args1.iter().zip(args2.iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::Trap { opcode: ref opcode1, code: ref code1 }, &Self::Trap { opcode: ref opcode2, code: ref code2 }) =>  {
                opcode1 == opcode2
                && code1 == code2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
            }
            (&Self::TryCall { opcode: ref opcode1, args: ref args1, func_ref: ref func_ref1, exception: ref exception1 }, &Self::TryCall { opcode: ref opcode2, args: ref args2, func_ref: ref func_ref2, exception: ref exception2 }) =>  {
                opcode1 == opcode2
                && func_ref1 == func_ref2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && exception1 == exception2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && args1.as_slice(pool).iter().zip(args2.as_slice(pool).iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::TryCallIndirect { opcode: ref opcode1, args: ref args1, exception: ref exception1 }, &Self::TryCallIndirect { opcode: ref opcode2, args: ref args2, exception: ref exception2 }) =>  {
                opcode1 == opcode2
                && exception1 == exception2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
                && args1.as_slice(pool).iter().zip(args2.as_slice(pool).iter()).all(|(a, b)| a == b) // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::Unary { opcode: ref opcode1, arg: ref arg1 }, &Self::Unary { opcode: ref opcode2, arg: ref arg2 }) =>  {
                opcode1 == opcode2
                && arg1 == arg2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:279
            }
            (&Self::UnaryConst { opcode: ref opcode1, constant_handle: ref constant_handle1 }, &Self::UnaryConst { opcode: ref opcode2, constant_handle: ref constant_handle2 }) =>  {
                opcode1 == opcode2
                && constant_handle1 == constant_handle2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
            }
            (&Self::UnaryGlobalValue { opcode: ref opcode1, global_value: ref global_value1 }, &Self::UnaryGlobalValue { opcode: ref opcode2, global_value: ref global_value2 }) =>  {
                opcode1 == opcode2
                && global_value1 == global_value2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
            }
            (&Self::UnaryIeee16 { opcode: ref opcode1, imm: ref imm1 }, &Self::UnaryIeee16 { opcode: ref opcode2, imm: ref imm2 }) =>  {
                opcode1 == opcode2
                && imm1 == imm2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
            }
            (&Self::UnaryIeee32 { opcode: ref opcode1, imm: ref imm1 }, &Self::UnaryIeee32 { opcode: ref opcode2, imm: ref imm2 }) =>  {
                opcode1 == opcode2
                && imm1 == imm2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
            }
            (&Self::UnaryIeee64 { opcode: ref opcode1, imm: ref imm1 }, &Self::UnaryIeee64 { opcode: ref opcode2, imm: ref imm2 }) =>  {
                opcode1 == opcode2
                && imm1 == imm2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
            }
            (&Self::UnaryImm { opcode: ref opcode1, imm: ref imm1 }, &Self::UnaryImm { opcode: ref opcode2, imm: ref imm2 }) =>  {
                opcode1 == opcode2
                && imm1 == imm2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:276
            }
            _ => unreachable!()
        }
    }

    /// Hash an `InstructionData`.
    ///
    /// This operation requires a reference to a `ValueListPool` to
    /// hash the contents of any `ValueLists`.
    ///
    /// This operation takes a closure that is allowed to map each
    /// argument value to some other value before it is hashed. This
    /// allows various forms of canonicalization.
    pub fn hash<H: ::core::hash::Hasher>(&self, state: &mut H, pool: &ir::ValueListPool) {
        match *self {
            Self::AtomicCas{opcode, ref args, flags} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&flags, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&args.len(), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::AtomicRmw{opcode, ref args, flags, op} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&flags, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&op, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&args.len(), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::Binary{opcode, ref args} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&args.len(), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::BinaryImm8{opcode, ref arg, imm} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&imm, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&1, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in core::slice::from_ref(arg) {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::BranchTable{opcode, ref arg, table} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&table, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&1, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in core::slice::from_ref(arg) {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::Brif{opcode, ref arg, ref blocks} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&1, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in core::slice::from_ref(arg) {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
                ::core::hash::Hash::hash(&blocks.len(), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:363
                for &block in blocks {
                    ::core::hash::Hash::hash(&block.block(pool), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:365
                    for arg in block.args(pool) {
                        ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:367
                    }
                }
            }
            Self::Call{opcode, ref args, func_ref} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&func_ref, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&args.len(pool), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args.as_slice(pool) {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::CallIndirect{opcode, ref args, sig_ref} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&sig_ref, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&args.len(pool), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args.as_slice(pool) {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::CondTrap{opcode, ref arg, code} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&code, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&1, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in core::slice::from_ref(arg) {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::DynamicStackAddr{opcode, dynamic_stack_slot} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&dynamic_stack_slot, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&0, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
            }
            Self::ExceptionHandlerAddress{opcode, block, imm} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&imm, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&0, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                ::core::hash::Hash::hash(&block, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:373
            }
            Self::FloatCompare{opcode, ref args, cond} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&cond, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&args.len(), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::FuncAddr{opcode, func_ref} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&func_ref, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&0, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
            }
            Self::IntAddTrap{opcode, ref args, code} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&code, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&args.len(), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::IntCompare{opcode, ref args, cond} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&cond, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&args.len(), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::Jump{opcode, ref destination} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&0, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                ::core::hash::Hash::hash(&1, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:363
                for &block in core::slice::from_ref(destination) {
                    ::core::hash::Hash::hash(&block.block(pool), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:365
                    for arg in block.args(pool) {
                        ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:367
                    }
                }
            }
            Self::Load{opcode, ref arg, flags, offset} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&flags, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&offset, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&1, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in core::slice::from_ref(arg) {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::LoadNoOffset{opcode, ref arg, flags} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&flags, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&1, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in core::slice::from_ref(arg) {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::MultiAry{opcode, ref args} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&args.len(pool), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args.as_slice(pool) {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::NullAry{opcode} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&0, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
            }
            Self::Shuffle{opcode, ref args, imm} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&imm, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&args.len(), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::StackAddr{opcode, stack_slot, offset} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&stack_slot, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&offset, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&0, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
            }
            Self::Store{opcode, ref args, flags, offset} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&flags, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&offset, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&args.len(), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::StoreNoOffset{opcode, ref args, flags} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&flags, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&args.len(), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::Ternary{opcode, ref args} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&args.len(), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::TernaryImm8{opcode, ref args, imm} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&imm, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&args.len(), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::Trap{opcode, code} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&code, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&0, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
            }
            Self::TryCall{opcode, ref args, func_ref, exception} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&func_ref, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&exception, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&args.len(pool), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args.as_slice(pool) {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::TryCallIndirect{opcode, ref args, exception} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&exception, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&args.len(pool), state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in args.as_slice(pool) {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::Unary{opcode, ref arg} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&1, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
                for &arg in core::slice::from_ref(arg) {
                    ::core::hash::Hash::hash(&arg, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:358
                }
            }
            Self::UnaryConst{opcode, constant_handle} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&constant_handle, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&0, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
            }
            Self::UnaryGlobalValue{opcode, global_value} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&global_value, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&0, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
            }
            Self::UnaryIeee16{opcode, imm} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&imm, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&0, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
            }
            Self::UnaryIeee32{opcode, imm} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&imm, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&0, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
            }
            Self::UnaryIeee64{opcode, imm} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&imm, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&0, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
            }
            Self::UnaryImm{opcode, imm} =>  {
                ::core::hash::Hash::hash( &::core::mem::discriminant(self), state);
                ::core::hash::Hash::hash(&opcode, state);
                ::core::hash::Hash::hash(&imm, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:353
                ::core::hash::Hash::hash(&0, state); // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:355
            }
        }
    }

    /// Deep-clone an `InstructionData`, including any referenced lists.
    ///
    /// This operation requires a reference to a `ValueListPool` to
    /// clone the `ValueLists`.
    pub fn deep_clone(&self, pool: &mut ir::ValueListPool) -> Self {
        match *self {
            Self::AtomicCas{opcode, args, flags} =>  {
                Self::AtomicCas {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:434
                    flags, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::AtomicRmw{opcode, args, flags, op} =>  {
                Self::AtomicRmw {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:434
                    flags, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                    op, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::Binary{opcode, args} =>  {
                Self::Binary {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:434
                }
            }
            Self::BinaryImm8{opcode, arg, imm} =>  {
                Self::BinaryImm8 {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    arg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:432
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::BranchTable{opcode, arg, table} =>  {
                Self::BranchTable {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    arg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:432
                    table, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::Brif{opcode, arg, blocks} =>  {
                Self::Brif {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    arg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:432
                    blocks: [blocks[0].deep_clone(pool), blocks[1].deep_clone(pool)], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:443
                }
            }
            Self::Call{opcode, ref args, func_ref} =>  {
                Self::Call {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args: args.deep_clone(pool), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:430
                    func_ref, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::CallIndirect{opcode, ref args, sig_ref} =>  {
                Self::CallIndirect {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args: args.deep_clone(pool), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:430
                    sig_ref, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::CondTrap{opcode, arg, code} =>  {
                Self::CondTrap {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    arg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:432
                    code, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::DynamicStackAddr{opcode, dynamic_stack_slot} =>  {
                Self::DynamicStackAddr {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    dynamic_stack_slot, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::ExceptionHandlerAddress{opcode, block, imm} =>  {
                Self::ExceptionHandlerAddress {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    block, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:451
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::FloatCompare{opcode, args, cond} =>  {
                Self::FloatCompare {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:434
                    cond, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::FuncAddr{opcode, func_ref} =>  {
                Self::FuncAddr {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    func_ref, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::IntAddTrap{opcode, args, code} =>  {
                Self::IntAddTrap {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:434
                    code, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::IntCompare{opcode, args, cond} =>  {
                Self::IntCompare {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:434
                    cond, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::Jump{opcode, destination} =>  {
                Self::Jump {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    destination: destination.deep_clone(pool), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:440
                }
            }
            Self::Load{opcode, arg, flags, offset} =>  {
                Self::Load {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    arg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:432
                    flags, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                    offset, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::LoadNoOffset{opcode, arg, flags} =>  {
                Self::LoadNoOffset {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    arg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:432
                    flags, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::MultiAry{opcode, ref args} =>  {
                Self::MultiAry {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args: args.deep_clone(pool), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:430
                }
            }
            Self::NullAry{opcode} =>  {
                Self::NullAry {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                }
            }
            Self::Shuffle{opcode, args, imm} =>  {
                Self::Shuffle {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:434
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::StackAddr{opcode, stack_slot, offset} =>  {
                Self::StackAddr {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    stack_slot, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                    offset, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::Store{opcode, args, flags, offset} =>  {
                Self::Store {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:434
                    flags, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                    offset, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::StoreNoOffset{opcode, args, flags} =>  {
                Self::StoreNoOffset {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:434
                    flags, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::Ternary{opcode, args} =>  {
                Self::Ternary {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:434
                }
            }
            Self::TernaryImm8{opcode, args, imm} =>  {
                Self::TernaryImm8 {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:434
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::Trap{opcode, code} =>  {
                Self::Trap {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    code, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::TryCall{opcode, ref args, func_ref, exception} =>  {
                Self::TryCall {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args: args.deep_clone(pool), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:430
                    func_ref, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                    exception, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::TryCallIndirect{opcode, ref args, exception} =>  {
                Self::TryCallIndirect {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    args: args.deep_clone(pool), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:430
                    exception, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::Unary{opcode, arg} =>  {
                Self::Unary {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    arg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:432
                }
            }
            Self::UnaryConst{opcode, constant_handle} =>  {
                Self::UnaryConst {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    constant_handle, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::UnaryGlobalValue{opcode, global_value} =>  {
                Self::UnaryGlobalValue {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    global_value, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::UnaryIeee16{opcode, imm} =>  {
                Self::UnaryIeee16 {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::UnaryIeee32{opcode, imm} =>  {
                Self::UnaryIeee32 {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::UnaryIeee64{opcode, imm} =>  {
                Self::UnaryIeee64 {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
            Self::UnaryImm{opcode, imm} =>  {
                Self::UnaryImm {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:427
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:457
                }
            }
        }
    }
    /// Map some functions, described by the given `InstructionMapper`, over each of the
    /// entities within this instruction, producing a new `InstructionData`.
    pub fn map(&self, mut mapper: impl crate::ir::instructions::InstructionMapper) -> Self {
        match *self {
            Self::AtomicCas{opcode, args, flags} =>  {
                Self::AtomicCas {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: [mapper.map_value(args[0]), mapper.map_value(args[1]), mapper.map_value(args[2])], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:518
                    flags: mapper.map_mem_flags(flags), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:563
                }
            }
            Self::AtomicRmw{opcode, args, flags, op} =>  {
                Self::AtomicRmw {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: [mapper.map_value(args[0]), mapper.map_value(args[1])], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:518
                    flags: mapper.map_mem_flags(flags), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:563
                    op, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::Binary{opcode, args} =>  {
                Self::Binary {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: [mapper.map_value(args[0]), mapper.map_value(args[1])], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:518
                }
            }
            Self::BinaryImm8{opcode, arg, imm} =>  {
                Self::BinaryImm8 {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    arg: mapper.map_value(arg), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:512
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::BranchTable{opcode, arg, table} =>  {
                Self::BranchTable {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    arg: mapper.map_value(arg), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:512
                    table: mapper.map_jump_table(table), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:555
                }
            }
            Self::Brif{opcode, arg, blocks} =>  {
                Self::Brif {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    arg: mapper.map_value(arg), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:512
                    blocks: [mapper.map_block_call(blocks[0]), mapper.map_block_call(blocks[1])], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:527
                }
            }
            Self::Call{opcode, args, func_ref} =>  {
                Self::Call {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: mapper.map_value_list(args), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:510
                    func_ref: mapper.map_func_ref(func_ref), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:555
                }
            }
            Self::CallIndirect{opcode, args, sig_ref} =>  {
                Self::CallIndirect {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: mapper.map_value_list(args), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:510
                    sig_ref: mapper.map_sig_ref(sig_ref), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:555
                }
            }
            Self::CondTrap{opcode, arg, code} =>  {
                Self::CondTrap {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    arg: mapper.map_value(arg), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:512
                    code, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::DynamicStackAddr{opcode, dynamic_stack_slot} =>  {
                Self::DynamicStackAddr {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    dynamic_stack_slot: mapper.map_dynamic_stack_slot(dynamic_stack_slot), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:555
                }
            }
            Self::ExceptionHandlerAddress{opcode, block, imm} =>  {
                Self::ExceptionHandlerAddress {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    block: mapper.map_block(block), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:535
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::FloatCompare{opcode, args, cond} =>  {
                Self::FloatCompare {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: [mapper.map_value(args[0]), mapper.map_value(args[1])], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:518
                    cond, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::FuncAddr{opcode, func_ref} =>  {
                Self::FuncAddr {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    func_ref: mapper.map_func_ref(func_ref), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:555
                }
            }
            Self::IntAddTrap{opcode, args, code} =>  {
                Self::IntAddTrap {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: [mapper.map_value(args[0]), mapper.map_value(args[1])], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:518
                    code, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::IntCompare{opcode, args, cond} =>  {
                Self::IntCompare {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: [mapper.map_value(args[0]), mapper.map_value(args[1])], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:518
                    cond, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::Jump{opcode, destination} =>  {
                Self::Jump {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    destination: mapper.map_block_call(destination), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:524
                }
            }
            Self::Load{opcode, arg, flags, offset} =>  {
                Self::Load {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    arg: mapper.map_value(arg), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:512
                    flags: mapper.map_mem_flags(flags), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:563
                    offset, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::LoadNoOffset{opcode, arg, flags} =>  {
                Self::LoadNoOffset {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    arg: mapper.map_value(arg), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:512
                    flags: mapper.map_mem_flags(flags), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:563
                }
            }
            Self::MultiAry{opcode, args} =>  {
                Self::MultiAry {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: mapper.map_value_list(args), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:510
                }
            }
            Self::NullAry{opcode} =>  {
                Self::NullAry {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                }
            }
            Self::Shuffle{opcode, args, imm} =>  {
                Self::Shuffle {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: [mapper.map_value(args[0]), mapper.map_value(args[1])], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:518
                    imm: mapper.map_immediate(imm), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:555
                }
            }
            Self::StackAddr{opcode, stack_slot, offset} =>  {
                Self::StackAddr {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    stack_slot: mapper.map_stack_slot(stack_slot), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:555
                    offset, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::Store{opcode, args, flags, offset} =>  {
                Self::Store {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: [mapper.map_value(args[0]), mapper.map_value(args[1])], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:518
                    flags: mapper.map_mem_flags(flags), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:563
                    offset, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::StoreNoOffset{opcode, args, flags} =>  {
                Self::StoreNoOffset {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: [mapper.map_value(args[0]), mapper.map_value(args[1])], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:518
                    flags: mapper.map_mem_flags(flags), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:563
                }
            }
            Self::Ternary{opcode, args} =>  {
                Self::Ternary {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: [mapper.map_value(args[0]), mapper.map_value(args[1]), mapper.map_value(args[2])], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:518
                }
            }
            Self::TernaryImm8{opcode, args, imm} =>  {
                Self::TernaryImm8 {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: [mapper.map_value(args[0]), mapper.map_value(args[1])], // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:518
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::Trap{opcode, code} =>  {
                Self::Trap {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    code, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::TryCall{opcode, args, func_ref, exception} =>  {
                Self::TryCall {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: mapper.map_value_list(args), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:510
                    func_ref: mapper.map_func_ref(func_ref), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:555
                    exception: mapper.map_exception_table(exception), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:555
                }
            }
            Self::TryCallIndirect{opcode, args, exception} =>  {
                Self::TryCallIndirect {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    args: mapper.map_value_list(args), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:510
                    exception: mapper.map_exception_table(exception), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:555
                }
            }
            Self::Unary{opcode, arg} =>  {
                Self::Unary {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    arg: mapper.map_value(arg), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:512
                }
            }
            Self::UnaryConst{opcode, constant_handle} =>  {
                Self::UnaryConst {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    constant_handle: mapper.map_constant(constant_handle), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:555
                }
            }
            Self::UnaryGlobalValue{opcode, global_value} =>  {
                Self::UnaryGlobalValue {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    global_value: mapper.map_global_value(global_value), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:555
                }
            }
            Self::UnaryIeee16{opcode, imm} =>  {
                Self::UnaryIeee16 {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::UnaryIeee32{opcode, imm} =>  {
                Self::UnaryIeee32 {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::UnaryIeee64{opcode, imm} =>  {
                Self::UnaryIeee64 {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
            Self::UnaryImm{opcode, imm} =>  {
                Self::UnaryImm {
                    opcode, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:507
                    imm, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:567
                }
            }
        }
    }
}

/// An instruction opcode.
///
/// All instructions from all supported ISAs are present.
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
#[cfg_attr(
            feature = "enable-serde",
            derive(serde_derive::Serialize, serde_derive::Deserialize)
        )]
pub enum Opcode {
    /// `jump block_call`. (Jump)
    Jump = 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:638
    /// `brif c, block_then, block_else`. (Brif)
    /// Type inferred from `c`.
    Brif, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `br_table x, JT`. (BranchTable)
    BrTable, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `debugtrap`. (NullAry)
    Debugtrap, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `trap code`. (Trap)
    Trap, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `trapz c, code`. (CondTrap)
    /// Type inferred from `c`.
    Trapz, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `trapnz c, code`. (CondTrap)
    /// Type inferred from `c`.
    Trapnz, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `return rvals`. (MultiAry)
    Return, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `rvals = call FN, args`. (Call)
    Call, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `rvals = call_indirect SIG, callee, args`. (CallIndirect)
    /// Type inferred from `callee`.
    CallIndirect, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `return_call FN, args`. (Call)
    ReturnCall, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `return_call_indirect SIG, callee, args`. (CallIndirect)
    /// Type inferred from `callee`.
    ReturnCallIndirect, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `addr = func_addr FN`. (FuncAddr)
    FuncAddr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `try_call callee, args, ET`. (TryCall)
    TryCall, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `try_call_indirect callee, args, ET`. (TryCallIndirect)
    /// Type inferred from `callee`.
    TryCallIndirect, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = splat x`. (Unary)
    Splat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = swizzle x, y`. (Binary)
    Swizzle, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = x86_pshufb x, y`. (Binary)
    X86Pshufb, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = insertlane x, y, Idx`. (TernaryImm8)
    /// Type inferred from `x`.
    Insertlane, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = extractlane x, Idx`. (BinaryImm8)
    /// Type inferred from `x`.
    Extractlane, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = smin x, y`. (Binary)
    /// Type inferred from `x`.
    Smin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = umin x, y`. (Binary)
    /// Type inferred from `x`.
    Umin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = smax x, y`. (Binary)
    /// Type inferred from `x`.
    Smax, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = umax x, y`. (Binary)
    /// Type inferred from `x`.
    Umax, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = avg_round x, y`. (Binary)
    /// Type inferred from `x`.
    AvgRound, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = uadd_sat x, y`. (Binary)
    /// Type inferred from `x`.
    UaddSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = sadd_sat x, y`. (Binary)
    /// Type inferred from `x`.
    SaddSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = usub_sat x, y`. (Binary)
    /// Type inferred from `x`.
    UsubSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = ssub_sat x, y`. (Binary)
    /// Type inferred from `x`.
    SsubSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = load MemFlags, p, Offset`. (Load)
    Load, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `store MemFlags, x, p, Offset`. (Store)
    /// Type inferred from `x`.
    Store, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = uload8 MemFlags, p, Offset`. (Load)
    Uload8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = sload8 MemFlags, p, Offset`. (Load)
    Sload8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `istore8 MemFlags, x, p, Offset`. (Store)
    /// Type inferred from `x`.
    Istore8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = uload16 MemFlags, p, Offset`. (Load)
    Uload16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = sload16 MemFlags, p, Offset`. (Load)
    Sload16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `istore16 MemFlags, x, p, Offset`. (Store)
    /// Type inferred from `x`.
    Istore16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = uload32 MemFlags, p, Offset`. (Load)
    /// Type inferred from `p`.
    Uload32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = sload32 MemFlags, p, Offset`. (Load)
    /// Type inferred from `p`.
    Sload32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `istore32 MemFlags, x, p, Offset`. (Store)
    /// Type inferred from `x`.
    Istore32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `out_payload0 = stack_switch store_context_ptr, load_context_ptr, in_payload0`. (Ternary)
    /// Type inferred from `load_context_ptr`.
    StackSwitch, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = uload8x8 MemFlags, p, Offset`. (Load)
    /// Type inferred from `p`.
    Uload8x8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = sload8x8 MemFlags, p, Offset`. (Load)
    /// Type inferred from `p`.
    Sload8x8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = uload16x4 MemFlags, p, Offset`. (Load)
    /// Type inferred from `p`.
    Uload16x4, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = sload16x4 MemFlags, p, Offset`. (Load)
    /// Type inferred from `p`.
    Sload16x4, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = uload32x2 MemFlags, p, Offset`. (Load)
    /// Type inferred from `p`.
    Uload32x2, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = sload32x2 MemFlags, p, Offset`. (Load)
    /// Type inferred from `p`.
    Sload32x2, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `addr = stack_addr SS, Offset`. (StackAddr)
    StackAddr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `addr = dynamic_stack_addr DSS`. (DynamicStackAddr)
    DynamicStackAddr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = symbol_value GV`. (UnaryGlobalValue)
    SymbolValue, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = tls_value GV`. (UnaryGlobalValue)
    TlsValue, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `addr = get_pinned_reg`. (NullAry)
    GetPinnedReg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `set_pinned_reg addr`. (Unary)
    /// Type inferred from `addr`.
    SetPinnedReg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `addr = get_frame_pointer`. (NullAry)
    GetFramePointer, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `addr = get_stack_pointer`. (NullAry)
    GetStackPointer, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `addr = get_return_address`. (NullAry)
    GetReturnAddress, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `addr = get_exception_handler_address block, index`. (ExceptionHandlerAddress)
    GetExceptionHandlerAddress, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = iconst N`. (UnaryImm)
    Iconst, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = f16const N`. (UnaryIeee16)
    F16const, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = f32const N`. (UnaryIeee32)
    F32const, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = f64const N`. (UnaryIeee64)
    F64const, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = f128const N`. (UnaryConst)
    F128const, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = vconst N`. (UnaryConst)
    Vconst, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = shuffle a, b, mask`. (Shuffle)
    Shuffle, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `nop`. (NullAry)
    Nop, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = select c, x, y`. (Ternary)
    /// Type inferred from `x`.
    Select, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = select_spectre_guard c, x, y`. (Ternary)
    /// Type inferred from `x`.
    SelectSpectreGuard, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = bitselect c, x, y`. (Ternary)
    /// Type inferred from `x`.
    Bitselect, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = blendv c, x, y`. (Ternary)
    /// Type inferred from `x`.
    Blendv, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `s = vany_true a`. (Unary)
    /// Type inferred from `a`.
    VanyTrue, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `s = vall_true a`. (Unary)
    /// Type inferred from `a`.
    VallTrue, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `x = vhigh_bits a`. (Unary)
    VhighBits, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = icmp Cond, x, y`. (IntCompare)
    /// Type inferred from `x`.
    Icmp, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = iadd x, y`. (Binary)
    /// Type inferred from `x`.
    Iadd, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = isub x, y`. (Binary)
    /// Type inferred from `x`.
    Isub, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = ineg x`. (Unary)
    /// Type inferred from `x`.
    Ineg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = iabs x`. (Unary)
    /// Type inferred from `x`.
    Iabs, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = imul x, y`. (Binary)
    /// Type inferred from `x`.
    Imul, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = umulhi x, y`. (Binary)
    /// Type inferred from `x`.
    Umulhi, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = smulhi x, y`. (Binary)
    /// Type inferred from `x`.
    Smulhi, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = sqmul_round_sat x, y`. (Binary)
    /// Type inferred from `x`.
    SqmulRoundSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = x86_pmulhrsw x, y`. (Binary)
    /// Type inferred from `x`.
    X86Pmulhrsw, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = udiv x, y`. (Binary)
    /// Type inferred from `x`.
    Udiv, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = sdiv x, y`. (Binary)
    /// Type inferred from `x`.
    Sdiv, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = urem x, y`. (Binary)
    /// Type inferred from `x`.
    Urem, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = srem x, y`. (Binary)
    /// Type inferred from `x`.
    Srem, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a, c_out = sadd_overflow_cin x, y, c_in`. (Ternary)
    /// Type inferred from `y`.
    SaddOverflowCin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a, c_out = uadd_overflow_cin x, y, c_in`. (Ternary)
    /// Type inferred from `y`.
    UaddOverflowCin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a, of = uadd_overflow x, y`. (Binary)
    /// Type inferred from `x`.
    UaddOverflow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a, of = sadd_overflow x, y`. (Binary)
    /// Type inferred from `x`.
    SaddOverflow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a, of = usub_overflow x, y`. (Binary)
    /// Type inferred from `x`.
    UsubOverflow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a, of = ssub_overflow x, y`. (Binary)
    /// Type inferred from `x`.
    SsubOverflow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a, of = umul_overflow x, y`. (Binary)
    /// Type inferred from `x`.
    UmulOverflow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a, of = smul_overflow x, y`. (Binary)
    /// Type inferred from `x`.
    SmulOverflow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = uadd_overflow_trap x, y, code`. (IntAddTrap)
    /// Type inferred from `x`.
    UaddOverflowTrap, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a, b_out = ssub_overflow_bin x, y, b_in`. (Ternary)
    /// Type inferred from `y`.
    SsubOverflowBin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a, b_out = usub_overflow_bin x, y, b_in`. (Ternary)
    /// Type inferred from `y`.
    UsubOverflowBin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = band x, y`. (Binary)
    /// Type inferred from `x`.
    Band, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = bor x, y`. (Binary)
    /// Type inferred from `x`.
    Bor, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = bxor x, y`. (Binary)
    /// Type inferred from `x`.
    Bxor, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = bnot x`. (Unary)
    /// Type inferred from `x`.
    Bnot, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = rotl x, y`. (Binary)
    /// Type inferred from `x`.
    Rotl, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = rotr x, y`. (Binary)
    /// Type inferred from `x`.
    Rotr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = ishl x, y`. (Binary)
    /// Type inferred from `x`.
    Ishl, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = ushr x, y`. (Binary)
    /// Type inferred from `x`.
    Ushr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = sshr x, y`. (Binary)
    /// Type inferred from `x`.
    Sshr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = bitrev x`. (Unary)
    /// Type inferred from `x`.
    Bitrev, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = clz x`. (Unary)
    /// Type inferred from `x`.
    Clz, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = cls x`. (Unary)
    /// Type inferred from `x`.
    Cls, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = ctz x`. (Unary)
    /// Type inferred from `x`.
    Ctz, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = bswap x`. (Unary)
    /// Type inferred from `x`.
    Bswap, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = popcnt x`. (Unary)
    /// Type inferred from `x`.
    Popcnt, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fcmp Cond, x, y`. (FloatCompare)
    /// Type inferred from `x`.
    Fcmp, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fadd x, y`. (Binary)
    /// Type inferred from `x`.
    Fadd, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fsub x, y`. (Binary)
    /// Type inferred from `x`.
    Fsub, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fmul x, y`. (Binary)
    /// Type inferred from `x`.
    Fmul, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fdiv x, y`. (Binary)
    /// Type inferred from `x`.
    Fdiv, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = sqrt x`. (Unary)
    /// Type inferred from `x`.
    Sqrt, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fma x, y, z`. (Ternary)
    /// Type inferred from `y`.
    Fma, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fneg x`. (Unary)
    /// Type inferred from `x`.
    Fneg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fabs x`. (Unary)
    /// Type inferred from `x`.
    Fabs, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fcopysign x, y`. (Binary)
    /// Type inferred from `x`.
    Fcopysign, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fmin x, y`. (Binary)
    /// Type inferred from `x`.
    Fmin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fmax x, y`. (Binary)
    /// Type inferred from `x`.
    Fmax, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = ceil x`. (Unary)
    /// Type inferred from `x`.
    Ceil, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = floor x`. (Unary)
    /// Type inferred from `x`.
    Floor, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = trunc x`. (Unary)
    /// Type inferred from `x`.
    Trunc, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = nearest x`. (Unary)
    /// Type inferred from `x`.
    Nearest, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = bitcast MemFlags, x`. (LoadNoOffset)
    Bitcast, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = scalar_to_vector s`. (Unary)
    ScalarToVector, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = bmask x`. (Unary)
    Bmask, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = ireduce x`. (Unary)
    Ireduce, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = snarrow x, y`. (Binary)
    /// Type inferred from `x`.
    Snarrow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = unarrow x, y`. (Binary)
    /// Type inferred from `x`.
    Unarrow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = uunarrow x, y`. (Binary)
    /// Type inferred from `x`.
    Uunarrow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = swiden_low x`. (Unary)
    /// Type inferred from `x`.
    SwidenLow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = swiden_high x`. (Unary)
    /// Type inferred from `x`.
    SwidenHigh, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = uwiden_low x`. (Unary)
    /// Type inferred from `x`.
    UwidenLow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = uwiden_high x`. (Unary)
    /// Type inferred from `x`.
    UwidenHigh, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = iadd_pairwise x, y`. (Binary)
    /// Type inferred from `x`.
    IaddPairwise, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = x86_pmaddubsw x, y`. (Binary)
    X86Pmaddubsw, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = uextend x`. (Unary)
    Uextend, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = sextend x`. (Unary)
    Sextend, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fpromote x`. (Unary)
    Fpromote, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fdemote x`. (Unary)
    Fdemote, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fvdemote x`. (Unary)
    Fvdemote, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `x = fvpromote_low a`. (Unary)
    FvpromoteLow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fcvt_to_uint x`. (Unary)
    FcvtToUint, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fcvt_to_sint x`. (Unary)
    FcvtToSint, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fcvt_to_uint_sat x`. (Unary)
    FcvtToUintSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fcvt_to_sint_sat x`. (Unary)
    FcvtToSintSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = x86_cvtt2dq x`. (Unary)
    X86Cvtt2dq, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fcvt_from_uint x`. (Unary)
    FcvtFromUint, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = fcvt_from_sint x`. (Unary)
    FcvtFromSint, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `lo, hi = isplit x`. (Unary)
    /// Type inferred from `x`.
    Isplit, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = iconcat lo, hi`. (Binary)
    /// Type inferred from `lo`.
    Iconcat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = atomic_rmw MemFlags, AtomicRmwOp, p, x`. (AtomicRmw)
    AtomicRmw, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = atomic_cas MemFlags, p, e, x`. (AtomicCas)
    /// Type inferred from `x`.
    AtomicCas, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = atomic_load MemFlags, p`. (LoadNoOffset)
    AtomicLoad, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `atomic_store MemFlags, x, p`. (StoreNoOffset)
    /// Type inferred from `x`.
    AtomicStore, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `fence`. (NullAry)
    Fence, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `a = extract_vector x, y`. (BinaryImm8)
    /// Type inferred from `x`.
    ExtractVector, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
    /// `sequence_point`. (NullAry)
    SequencePoint, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:641
}

impl Opcode {
    /// True for instructions that terminate the block
    pub fn is_terminator(self) -> bool {
        match self {
            Self::BrTable |
            Self::Brif |
            Self::Jump |
            Self::Return |
            Self::ReturnCall |
            Self::ReturnCallIndirect |
            Self::Trap |
            Self::TryCall |
            Self::TryCallIndirect => {
                true
            }
            _ => {
                false
            }
        }
    }

    /// True for all branch or jump instructions.
    pub fn is_branch(self) -> bool {
        match self {
            Self::BrTable |
            Self::Brif |
            Self::Jump |
            Self::TryCall |
            Self::TryCallIndirect => {
                true
            }
            _ => {
                false
            }
        }
    }

    /// Is this a call instruction?
    pub fn is_call(self) -> bool {
        match self {
            Self::Call |
            Self::CallIndirect |
            Self::ReturnCall |
            Self::ReturnCallIndirect |
            Self::StackSwitch |
            Self::TryCall |
            Self::TryCallIndirect => {
                true
            }
            _ => {
                false
            }
        }
    }

    /// Is this a return instruction?
    pub fn is_return(self) -> bool {
        match self {
            Self::Return |
            Self::ReturnCall |
            Self::ReturnCallIndirect => {
                true
            }
            _ => {
                false
            }
        }
    }

    /// Can this instruction read from memory?
    pub fn can_load(self) -> bool {
        match self {
            Self::AtomicCas |
            Self::AtomicLoad |
            Self::AtomicRmw |
            Self::Debugtrap |
            Self::Load |
            Self::Sload16 |
            Self::Sload16x4 |
            Self::Sload32 |
            Self::Sload32x2 |
            Self::Sload8 |
            Self::Sload8x8 |
            Self::StackSwitch |
            Self::Uload16 |
            Self::Uload16x4 |
            Self::Uload32 |
            Self::Uload32x2 |
            Self::Uload8 |
            Self::Uload8x8 => {
                true
            }
            _ => {
                false
            }
        }
    }

    /// Can this instruction write to memory?
    pub fn can_store(self) -> bool {
        match self {
            Self::AtomicCas |
            Self::AtomicRmw |
            Self::AtomicStore |
            Self::Debugtrap |
            Self::Istore16 |
            Self::Istore32 |
            Self::Istore8 |
            Self::StackSwitch |
            Self::Store => {
                true
            }
            _ => {
                false
            }
        }
    }

    /// Can this instruction cause a trap?
    pub fn can_trap(self) -> bool {
        match self {
            Self::FcvtToSint |
            Self::FcvtToUint |
            Self::Sdiv |
            Self::Srem |
            Self::Trap |
            Self::Trapnz |
            Self::Trapz |
            Self::UaddOverflowTrap |
            Self::Udiv |
            Self::Urem => {
                true
            }
            _ => {
                false
            }
        }
    }

    /// Does this instruction have other side effects besides can_* flags?
    pub fn other_side_effects(self) -> bool {
        match self {
            Self::AtomicCas |
            Self::AtomicLoad |
            Self::AtomicRmw |
            Self::AtomicStore |
            Self::Debugtrap |
            Self::Fence |
            Self::GetPinnedReg |
            Self::SequencePoint |
            Self::SetPinnedReg |
            Self::StackSwitch => {
                true
            }
            _ => {
                false
            }
        }
    }

    /// Despite having side effects, is this instruction okay to GVN?
    pub fn side_effects_idempotent(self) -> bool {
        match self {
            Self::FcvtToSint |
            Self::FcvtToUint |
            Self::Sdiv |
            Self::Srem |
            Self::Trapnz |
            Self::Trapz |
            Self::UaddOverflowTrap |
            Self::Udiv |
            Self::Urem => {
                true
            }
            _ => {
                false
            }
        }
    }

    /// All cranelift opcodes.
    pub fn all() -> &'static [Opcode] {
        return &[
            Opcode::Jump, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Brif, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::BrTable, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Debugtrap, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Trap, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Trapz, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Trapnz, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Return, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Call, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::CallIndirect, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::ReturnCall, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::ReturnCallIndirect, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::FuncAddr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::TryCall, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::TryCallIndirect, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Splat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Swizzle, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::X86Pshufb, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Insertlane, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Extractlane, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Smin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Umin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Smax, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Umax, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::AvgRound, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::UaddSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SaddSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::UsubSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SsubSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Load, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Store, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Uload8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Sload8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Istore8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Uload16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Sload16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Istore16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Uload32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Sload32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Istore32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::StackSwitch, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Uload8x8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Sload8x8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Uload16x4, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Sload16x4, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Uload32x2, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Sload32x2, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::StackAddr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::DynamicStackAddr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SymbolValue, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::TlsValue, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::GetPinnedReg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SetPinnedReg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::GetFramePointer, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::GetStackPointer, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::GetReturnAddress, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::GetExceptionHandlerAddress, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Iconst, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::F16const, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::F32const, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::F64const, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::F128const, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Vconst, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Shuffle, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Nop, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Select, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SelectSpectreGuard, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Bitselect, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Blendv, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::VanyTrue, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::VallTrue, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::VhighBits, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Icmp, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Iadd, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Isub, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Ineg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Iabs, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Imul, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Umulhi, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Smulhi, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SqmulRoundSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::X86Pmulhrsw, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Udiv, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Sdiv, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Urem, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Srem, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SaddOverflowCin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::UaddOverflowCin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::UaddOverflow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SaddOverflow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::UsubOverflow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SsubOverflow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::UmulOverflow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SmulOverflow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::UaddOverflowTrap, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SsubOverflowBin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::UsubOverflowBin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Band, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Bor, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Bxor, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Bnot, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Rotl, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Rotr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Ishl, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Ushr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Sshr, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Bitrev, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Clz, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Cls, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Ctz, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Bswap, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Popcnt, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fcmp, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fadd, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fsub, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fmul, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fdiv, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Sqrt, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fma, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fneg, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fabs, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fcopysign, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fmin, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fmax, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Ceil, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Floor, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Trunc, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Nearest, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Bitcast, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::ScalarToVector, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Bmask, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Ireduce, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Snarrow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Unarrow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Uunarrow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SwidenLow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SwidenHigh, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::UwidenLow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::UwidenHigh, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::IaddPairwise, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::X86Pmaddubsw, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Uextend, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Sextend, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fpromote, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fdemote, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fvdemote, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::FvpromoteLow, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::FcvtToUint, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::FcvtToSint, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::FcvtToUintSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::FcvtToSintSat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::X86Cvtt2dq, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::FcvtFromUint, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::FcvtFromSint, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Isplit, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Iconcat, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::AtomicRmw, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::AtomicCas, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::AtomicLoad, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::AtomicStore, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::Fence, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::ExtractVector, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
            Opcode::SequencePoint, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:718
        ];
    }

}

const OPCODE_FORMAT: [InstructionFormat; 163] = [ // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:728
    InstructionFormat::Jump, // jump // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Brif, // brif // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::BranchTable, // br_table // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::NullAry, // debugtrap // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Trap, // trap // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::CondTrap, // trapz // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::CondTrap, // trapnz // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::MultiAry, // return // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Call, // call // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::CallIndirect, // call_indirect // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Call, // return_call // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::CallIndirect, // return_call_indirect // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::FuncAddr, // func_addr // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::TryCall, // try_call // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::TryCallIndirect, // try_call_indirect // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // splat // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // swizzle // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // x86_pshufb // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::TernaryImm8, // insertlane // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::BinaryImm8, // extractlane // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // smin // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // umin // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // smax // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // umax // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // avg_round // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // uadd_sat // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // sadd_sat // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // usub_sat // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // ssub_sat // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Load, // load // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Store, // store // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Load, // uload8 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Load, // sload8 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Store, // istore8 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Load, // uload16 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Load, // sload16 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Store, // istore16 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Load, // uload32 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Load, // sload32 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Store, // istore32 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Ternary, // stack_switch // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Load, // uload8x8 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Load, // sload8x8 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Load, // uload16x4 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Load, // sload16x4 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Load, // uload32x2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Load, // sload32x2 // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::StackAddr, // stack_addr // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::DynamicStackAddr, // dynamic_stack_addr // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::UnaryGlobalValue, // symbol_value // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::UnaryGlobalValue, // tls_value // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::NullAry, // get_pinned_reg // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // set_pinned_reg // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::NullAry, // get_frame_pointer // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::NullAry, // get_stack_pointer // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::NullAry, // get_return_address // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::ExceptionHandlerAddress, // get_exception_handler_address // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::UnaryImm, // iconst // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::UnaryIeee16, // f16const // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::UnaryIeee32, // f32const // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::UnaryIeee64, // f64const // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::UnaryConst, // f128const // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::UnaryConst, // vconst // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Shuffle, // shuffle // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::NullAry, // nop // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Ternary, // select // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Ternary, // select_spectre_guard // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Ternary, // bitselect // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Ternary, // blendv // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // vany_true // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // vall_true // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // vhigh_bits // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::IntCompare, // icmp // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // iadd // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // isub // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // ineg // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // iabs // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // imul // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // umulhi // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // smulhi // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // sqmul_round_sat // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // x86_pmulhrsw // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // udiv // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // sdiv // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // urem // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // srem // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Ternary, // sadd_overflow_cin // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Ternary, // uadd_overflow_cin // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // uadd_overflow // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // sadd_overflow // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // usub_overflow // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // ssub_overflow // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // umul_overflow // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // smul_overflow // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::IntAddTrap, // uadd_overflow_trap // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Ternary, // ssub_overflow_bin // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Ternary, // usub_overflow_bin // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // band // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // bor // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // bxor // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // bnot // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // rotl // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // rotr // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // ishl // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // ushr // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // sshr // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // bitrev // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // clz // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // cls // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // ctz // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // bswap // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // popcnt // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::FloatCompare, // fcmp // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // fadd // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // fsub // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // fmul // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // fdiv // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // sqrt // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Ternary, // fma // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // fneg // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // fabs // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // fcopysign // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // fmin // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // fmax // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // ceil // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // floor // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // trunc // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // nearest // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::LoadNoOffset, // bitcast // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // scalar_to_vector // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // bmask // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // ireduce // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // snarrow // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // unarrow // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // uunarrow // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // swiden_low // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // swiden_high // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // uwiden_low // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // uwiden_high // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // iadd_pairwise // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // x86_pmaddubsw // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // uextend // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // sextend // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // fpromote // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // fdemote // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // fvdemote // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // fvpromote_low // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // fcvt_to_uint // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // fcvt_to_sint // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // fcvt_to_uint_sat // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // fcvt_to_sint_sat // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // x86_cvtt2dq // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // fcvt_from_uint // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // fcvt_from_sint // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Unary, // isplit // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::Binary, // iconcat // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::AtomicRmw, // atomic_rmw // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::AtomicCas, // atomic_cas // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::LoadNoOffset, // atomic_load // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::StoreNoOffset, // atomic_store // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::NullAry, // fence // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::BinaryImm8, // extract_vector // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
    InstructionFormat::NullAry, // sequence_point // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:735
]; // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:743

fn opcode_name(opc: Opcode) -> &'static str {
    match opc {
        Opcode::AtomicCas => {
            "atomic_cas"
        }
        Opcode::AtomicLoad => {
            "atomic_load"
        }
        Opcode::AtomicRmw => {
            "atomic_rmw"
        }
        Opcode::AtomicStore => {
            "atomic_store"
        }
        Opcode::AvgRound => {
            "avg_round"
        }
        Opcode::Band => {
            "band"
        }
        Opcode::Bitcast => {
            "bitcast"
        }
        Opcode::Bitrev => {
            "bitrev"
        }
        Opcode::Bitselect => {
            "bitselect"
        }
        Opcode::Blendv => {
            "blendv"
        }
        Opcode::Bmask => {
            "bmask"
        }
        Opcode::Bnot => {
            "bnot"
        }
        Opcode::Bor => {
            "bor"
        }
        Opcode::BrTable => {
            "br_table"
        }
        Opcode::Brif => {
            "brif"
        }
        Opcode::Bswap => {
            "bswap"
        }
        Opcode::Bxor => {
            "bxor"
        }
        Opcode::Call => {
            "call"
        }
        Opcode::CallIndirect => {
            "call_indirect"
        }
        Opcode::Ceil => {
            "ceil"
        }
        Opcode::Cls => {
            "cls"
        }
        Opcode::Clz => {
            "clz"
        }
        Opcode::Ctz => {
            "ctz"
        }
        Opcode::Debugtrap => {
            "debugtrap"
        }
        Opcode::DynamicStackAddr => {
            "dynamic_stack_addr"
        }
        Opcode::ExtractVector => {
            "extract_vector"
        }
        Opcode::Extractlane => {
            "extractlane"
        }
        Opcode::F128const => {
            "f128const"
        }
        Opcode::F16const => {
            "f16const"
        }
        Opcode::F32const => {
            "f32const"
        }
        Opcode::F64const => {
            "f64const"
        }
        Opcode::Fabs => {
            "fabs"
        }
        Opcode::Fadd => {
            "fadd"
        }
        Opcode::Fcmp => {
            "fcmp"
        }
        Opcode::Fcopysign => {
            "fcopysign"
        }
        Opcode::FcvtFromSint => {
            "fcvt_from_sint"
        }
        Opcode::FcvtFromUint => {
            "fcvt_from_uint"
        }
        Opcode::FcvtToSint => {
            "fcvt_to_sint"
        }
        Opcode::FcvtToSintSat => {
            "fcvt_to_sint_sat"
        }
        Opcode::FcvtToUint => {
            "fcvt_to_uint"
        }
        Opcode::FcvtToUintSat => {
            "fcvt_to_uint_sat"
        }
        Opcode::Fdemote => {
            "fdemote"
        }
        Opcode::Fdiv => {
            "fdiv"
        }
        Opcode::Fence => {
            "fence"
        }
        Opcode::Floor => {
            "floor"
        }
        Opcode::Fma => {
            "fma"
        }
        Opcode::Fmax => {
            "fmax"
        }
        Opcode::Fmin => {
            "fmin"
        }
        Opcode::Fmul => {
            "fmul"
        }
        Opcode::Fneg => {
            "fneg"
        }
        Opcode::Fpromote => {
            "fpromote"
        }
        Opcode::Fsub => {
            "fsub"
        }
        Opcode::FuncAddr => {
            "func_addr"
        }
        Opcode::Fvdemote => {
            "fvdemote"
        }
        Opcode::FvpromoteLow => {
            "fvpromote_low"
        }
        Opcode::GetExceptionHandlerAddress => {
            "get_exception_handler_address"
        }
        Opcode::GetFramePointer => {
            "get_frame_pointer"
        }
        Opcode::GetPinnedReg => {
            "get_pinned_reg"
        }
        Opcode::GetReturnAddress => {
            "get_return_address"
        }
        Opcode::GetStackPointer => {
            "get_stack_pointer"
        }
        Opcode::Iabs => {
            "iabs"
        }
        Opcode::Iadd => {
            "iadd"
        }
        Opcode::IaddPairwise => {
            "iadd_pairwise"
        }
        Opcode::Icmp => {
            "icmp"
        }
        Opcode::Iconcat => {
            "iconcat"
        }
        Opcode::Iconst => {
            "iconst"
        }
        Opcode::Imul => {
            "imul"
        }
        Opcode::Ineg => {
            "ineg"
        }
        Opcode::Insertlane => {
            "insertlane"
        }
        Opcode::Ireduce => {
            "ireduce"
        }
        Opcode::Ishl => {
            "ishl"
        }
        Opcode::Isplit => {
            "isplit"
        }
        Opcode::Istore16 => {
            "istore16"
        }
        Opcode::Istore32 => {
            "istore32"
        }
        Opcode::Istore8 => {
            "istore8"
        }
        Opcode::Isub => {
            "isub"
        }
        Opcode::Jump => {
            "jump"
        }
        Opcode::Load => {
            "load"
        }
        Opcode::Nearest => {
            "nearest"
        }
        Opcode::Nop => {
            "nop"
        }
        Opcode::Popcnt => {
            "popcnt"
        }
        Opcode::Return => {
            "return"
        }
        Opcode::ReturnCall => {
            "return_call"
        }
        Opcode::ReturnCallIndirect => {
            "return_call_indirect"
        }
        Opcode::Rotl => {
            "rotl"
        }
        Opcode::Rotr => {
            "rotr"
        }
        Opcode::SaddOverflow => {
            "sadd_overflow"
        }
        Opcode::SaddOverflowCin => {
            "sadd_overflow_cin"
        }
        Opcode::SaddSat => {
            "sadd_sat"
        }
        Opcode::ScalarToVector => {
            "scalar_to_vector"
        }
        Opcode::Sdiv => {
            "sdiv"
        }
        Opcode::Select => {
            "select"
        }
        Opcode::SelectSpectreGuard => {
            "select_spectre_guard"
        }
        Opcode::SequencePoint => {
            "sequence_point"
        }
        Opcode::SetPinnedReg => {
            "set_pinned_reg"
        }
        Opcode::Sextend => {
            "sextend"
        }
        Opcode::Shuffle => {
            "shuffle"
        }
        Opcode::Sload16 => {
            "sload16"
        }
        Opcode::Sload16x4 => {
            "sload16x4"
        }
        Opcode::Sload32 => {
            "sload32"
        }
        Opcode::Sload32x2 => {
            "sload32x2"
        }
        Opcode::Sload8 => {
            "sload8"
        }
        Opcode::Sload8x8 => {
            "sload8x8"
        }
        Opcode::Smax => {
            "smax"
        }
        Opcode::Smin => {
            "smin"
        }
        Opcode::SmulOverflow => {
            "smul_overflow"
        }
        Opcode::Smulhi => {
            "smulhi"
        }
        Opcode::Snarrow => {
            "snarrow"
        }
        Opcode::Splat => {
            "splat"
        }
        Opcode::SqmulRoundSat => {
            "sqmul_round_sat"
        }
        Opcode::Sqrt => {
            "sqrt"
        }
        Opcode::Srem => {
            "srem"
        }
        Opcode::Sshr => {
            "sshr"
        }
        Opcode::SsubOverflow => {
            "ssub_overflow"
        }
        Opcode::SsubOverflowBin => {
            "ssub_overflow_bin"
        }
        Opcode::SsubSat => {
            "ssub_sat"
        }
        Opcode::StackAddr => {
            "stack_addr"
        }
        Opcode::StackSwitch => {
            "stack_switch"
        }
        Opcode::Store => {
            "store"
        }
        Opcode::SwidenHigh => {
            "swiden_high"
        }
        Opcode::SwidenLow => {
            "swiden_low"
        }
        Opcode::Swizzle => {
            "swizzle"
        }
        Opcode::SymbolValue => {
            "symbol_value"
        }
        Opcode::TlsValue => {
            "tls_value"
        }
        Opcode::Trap => {
            "trap"
        }
        Opcode::Trapnz => {
            "trapnz"
        }
        Opcode::Trapz => {
            "trapz"
        }
        Opcode::Trunc => {
            "trunc"
        }
        Opcode::TryCall => {
            "try_call"
        }
        Opcode::TryCallIndirect => {
            "try_call_indirect"
        }
        Opcode::UaddOverflow => {
            "uadd_overflow"
        }
        Opcode::UaddOverflowCin => {
            "uadd_overflow_cin"
        }
        Opcode::UaddOverflowTrap => {
            "uadd_overflow_trap"
        }
        Opcode::UaddSat => {
            "uadd_sat"
        }
        Opcode::Udiv => {
            "udiv"
        }
        Opcode::Uextend => {
            "uextend"
        }
        Opcode::Uload16 => {
            "uload16"
        }
        Opcode::Uload16x4 => {
            "uload16x4"
        }
        Opcode::Uload32 => {
            "uload32"
        }
        Opcode::Uload32x2 => {
            "uload32x2"
        }
        Opcode::Uload8 => {
            "uload8"
        }
        Opcode::Uload8x8 => {
            "uload8x8"
        }
        Opcode::Umax => {
            "umax"
        }
        Opcode::Umin => {
            "umin"
        }
        Opcode::UmulOverflow => {
            "umul_overflow"
        }
        Opcode::Umulhi => {
            "umulhi"
        }
        Opcode::Unarrow => {
            "unarrow"
        }
        Opcode::Urem => {
            "urem"
        }
        Opcode::Ushr => {
            "ushr"
        }
        Opcode::UsubOverflow => {
            "usub_overflow"
        }
        Opcode::UsubOverflowBin => {
            "usub_overflow_bin"
        }
        Opcode::UsubSat => {
            "usub_sat"
        }
        Opcode::Uunarrow => {
            "uunarrow"
        }
        Opcode::UwidenHigh => {
            "uwiden_high"
        }
        Opcode::UwidenLow => {
            "uwiden_low"
        }
        Opcode::VallTrue => {
            "vall_true"
        }
        Opcode::VanyTrue => {
            "vany_true"
        }
        Opcode::Vconst => {
            "vconst"
        }
        Opcode::VhighBits => {
            "vhigh_bits"
        }
        Opcode::X86Cvtt2dq => {
            "x86_cvtt2dq"
        }
        Opcode::X86Pmaddubsw => {
            "x86_pmaddubsw"
        }
        Opcode::X86Pmulhrsw => {
            "x86_pmulhrsw"
        }
        Opcode::X86Pshufb => {
            "x86_pshufb"
        }
    }
}

const OPCODE_HASH_TABLE: [Option<Opcode>; 256] = [ // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:764
    Some(Opcode::Imul), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::TlsValue), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Brif), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Nearest), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::FcvtToSintSat), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Fsub), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Trunc), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Urem), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Iconst), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::ReturnCall), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Umin), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Store), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::GetFramePointer), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Isub), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::FcvtFromSint), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Trap), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Sdiv), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Srem), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Uunarrow), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::UaddOverflowCin), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Bxor), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::X86Pmaddubsw), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Umax), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::FcvtFromUint), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Insertlane), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Fadd), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Swizzle), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Load), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Jump), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Shuffle), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Fneg), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Umulhi), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Ushr), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::UaddOverflowTrap), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::VallTrue), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Band), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::SsubOverflow), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Uload16x4), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Ishl), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Fmax), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Vconst), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Call), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::ExtractVector), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Sqrt), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Ceil), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Ineg), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::FuncAddr), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::SaddSat), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Popcnt), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Fabs), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Fmin), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::SsubOverflowBin), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::FcvtToUint), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Bnot), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Sextend), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Isplit), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Fdiv), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Fcmp), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::SwidenHigh), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Fmul), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::FcvtToSint), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::UsubOverflow), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Uload8x8), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::AtomicLoad), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Trapnz), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Uload16), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Uload32), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Bitrev), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Smulhi), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::TryCall), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Blendv), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Sload8x8), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::SetPinnedReg), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Ireduce), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Fdemote), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::FvpromoteLow), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::UwidenLow), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Select), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Istore32), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Istore16), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Fvdemote), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Sload16), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Fcopysign), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Unarrow), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::AvgRound), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Sload32), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::X86Pshufb), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Extractlane), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::StackAddr), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::SaddOverflowCin), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::UaddOverflow), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Return), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Uload32x2), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::VanyTrue), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::UsubSat), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::GetExceptionHandlerAddress), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Iconcat), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::SmulOverflow), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Fence), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Fma), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Bitselect), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Istore8), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::BrTable), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::F64const), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::StackSwitch), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Nop), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Bor), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Clz), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::SqmulRoundSat), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::X86Pmulhrsw), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Debugtrap), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Sload16x4), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::UmulOverflow), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Cls), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::SaddOverflow), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Ctz), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::SequencePoint), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::TryCallIndirect), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::UwidenHigh), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Bitcast), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Uextend), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Floor), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::UaddSat), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Sload32x2), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::SelectSpectreGuard), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Fpromote), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::SymbolValue), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::DynamicStackAddr), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Bmask), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::GetPinnedReg), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::SsubSat), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::AtomicRmw), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::ScalarToVector), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Uload8), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::FcvtToUintSat), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Smin), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Trapz), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Iabs), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::F16const), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Udiv), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::AtomicCas), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::GetReturnAddress), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::UsubOverflowBin), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::SwidenLow), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::ReturnCallIndirect), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Rotl), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::IaddPairwise), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Smax), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::F128const), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::F32const), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Splat), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Rotr), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Snarrow), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::CallIndirect), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Sload8), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::X86Cvtt2dq), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::VhighBits), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Iadd), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Icmp), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::GetStackPointer), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::Bswap), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
    Some(Opcode::Sshr), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    Some(Opcode::AtomicStore), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:772
    None, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:773
]; // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:777


// Table of opcode constraints.
const OPCODE_CONSTRAINTS: [OpcodeConstraints; 163] = [ // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:898
    // Jump: fixed_results=0, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=[]
    OpcodeConstraints {
        flags: 0x00, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Brif: fixed_results=0, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x38, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // BrTable: fixed_results=0, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Concrete(ir::types::I32)']
    OpcodeConstraints {
        flags: 0x20, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 3, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Debugtrap: fixed_results=0, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=[]
    OpcodeConstraints {
        flags: 0x00, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Trap: fixed_results=0, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=[]
    OpcodeConstraints {
        flags: 0x00, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Trapz: fixed_results=0, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x38, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Trapnz: fixed_results=0, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x38, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Return: fixed_results=0, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=[]
    OpcodeConstraints {
        flags: 0x00, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Call: fixed_results=0, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=[]
    OpcodeConstraints {
        flags: 0x00, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // CallIndirect: fixed_results=0, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x38, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // ReturnCall: fixed_results=0, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=[]
    OpcodeConstraints {
        flags: 0x00, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // ReturnCallIndirect: fixed_results=0, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x38, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // FuncAddr: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // TryCall: fixed_results=0, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=[]
    OpcodeConstraints {
        flags: 0x00, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // TryCallIndirect: fixed_results=0, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x38, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Splat: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'LaneOf']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 2, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 4, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Swizzle: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Concrete(ir::types::I8X16)', 'Concrete(ir::types::I8X16)', 'Concrete(ir::types::I8X16)']
    OpcodeConstraints {
        flags: 0x41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 6, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // X86Pshufb: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Concrete(ir::types::I8X16)', 'Concrete(ir::types::I8X16)', 'Concrete(ir::types::I8X16)']
    OpcodeConstraints {
        flags: 0x41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 6, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Insertlane: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'LaneOf']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 2, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 9, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Extractlane: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['LaneOf', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 2, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Smin: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 3, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Umin: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 3, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Smax: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 3, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Umax: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 3, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // AvgRound: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 4, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // UaddSat: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 4, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SaddSat: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 4, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // UsubSat: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 4, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SsubSat: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 4, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Load: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(1)']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 5, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 12, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Store: fixed_results=0, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=2
    // Constraints=['Same', 'Free(1)']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x58, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 5, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 12, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Uload8: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(1)']
    // Polymorphic over TypeSet(lanes={1}, ints={16, 32, 64})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 6, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 12, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Sload8: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(1)']
    // Polymorphic over TypeSet(lanes={1}, ints={16, 32, 64})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 6, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 12, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Istore8: fixed_results=0, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=2
    // Constraints=['Same', 'Free(1)']
    // Polymorphic over TypeSet(lanes={1}, ints={16, 32, 64})
    OpcodeConstraints {
        flags: 0x58, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 6, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 12, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Uload16: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(1)']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 12, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Sload16: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(1)']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 12, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Istore16: fixed_results=0, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=2
    // Constraints=['Same', 'Free(1)']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x58, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 12, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Uload32: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Concrete(ir::types::I64)', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Sload32: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Concrete(ir::types::I64)', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Istore32: fixed_results=0, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=2
    // Constraints=['Concrete(ir::types::I64)', 'Free(1)']
    // Polymorphic over TypeSet(lanes={1}, ints={64})
    OpcodeConstraints {
        flags: 0x58, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 7, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // StackSwitch: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=3
    // Constraints=['Same', 'Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x69, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 18, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Uload8x8: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Concrete(ir::types::I16X8)', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 22, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Sload8x8: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Concrete(ir::types::I16X8)', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 22, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Uload16x4: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Concrete(ir::types::I32X4)', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 24, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Sload16x4: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Concrete(ir::types::I32X4)', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 24, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Uload32x2: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Concrete(ir::types::I64X2)', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 26, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Sload32x2: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Concrete(ir::types::I64X2)', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 26, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // StackAddr: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // DynamicStackAddr: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SymbolValue: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 5, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // TlsValue: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 5, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // GetPinnedReg: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SetPinnedReg: fixed_results=0, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x38, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // GetFramePointer: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // GetStackPointer: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // GetReturnAddress: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // GetExceptionHandlerAddress: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Iconst: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64})
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // F16const: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Concrete(ir::types::F16)']
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 28, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // F32const: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Concrete(ir::types::F32)']
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // F64const: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Concrete(ir::types::F64)']
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 30, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // F128const: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Concrete(ir::types::F128)']
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 31, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Vconst: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=['Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x01, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 9, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Shuffle: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Concrete(ir::types::I8X16)', 'Concrete(ir::types::I8X16)', 'Concrete(ir::types::I8X16)']
    OpcodeConstraints {
        flags: 0x41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 6, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Nop: fixed_results=0, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=[]
    OpcodeConstraints {
        flags: 0x00, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Select: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=3
    // Constraints=['Same', 'Free(0)', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x69, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 10, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SelectSpectreGuard: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=3
    // Constraints=['Same', 'Free(0)', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x69, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 10, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Bitselect: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=3
    // Constraints=['Same', 'Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x69, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 10, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 18, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Blendv: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=3
    // Constraints=['Same', 'Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x69, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 10, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 18, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // VanyTrue: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Concrete(ir::types::I8)', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 9, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 36, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // VallTrue: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['Concrete(ir::types::I8)', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 9, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 36, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // VhighBits: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(9)']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 37, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Icmp: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=2
    // Constraints=['AsTruthy', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x59, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Iadd: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Isub: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Ineg: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Iabs: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Imul: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Umulhi: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Smulhi: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SqmulRoundSat: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={4, 8}, ints={16, 32})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 12, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // X86Pmulhrsw: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={4, 8}, ints={16, 32})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 12, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Udiv: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Sdiv: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Urem: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Srem: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SaddOverflowCin: fixed_results=2, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=3
    // Constraints=['Same', 'Concrete(ir::types::I8)', 'Same', 'Same', 'Concrete(ir::types::I8)']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x6a, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // UaddOverflowCin: fixed_results=2, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=3
    // Constraints=['Same', 'Concrete(ir::types::I8)', 'Same', 'Same', 'Concrete(ir::types::I8)']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x6a, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // UaddOverflow: fixed_results=2, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Concrete(ir::types::I8)', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x4a, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SaddOverflow: fixed_results=2, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Concrete(ir::types::I8)', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x4a, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // UsubOverflow: fixed_results=2, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Concrete(ir::types::I8)', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x4a, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SsubOverflow: fixed_results=2, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Concrete(ir::types::I8)', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x4a, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // UmulOverflow: fixed_results=2, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Concrete(ir::types::I8)', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64})
    OpcodeConstraints {
        flags: 0x4a, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SmulOverflow: fixed_results=2, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Concrete(ir::types::I8)', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64})
    OpcodeConstraints {
        flags: 0x4a, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // UaddOverflowTrap: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={32, 64})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 1, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SsubOverflowBin: fixed_results=2, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=3
    // Constraints=['Same', 'Concrete(ir::types::I8)', 'Same', 'Same', 'Concrete(ir::types::I8)']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x6a, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // UsubOverflowBin: fixed_results=2, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=3
    // Constraints=['Same', 'Concrete(ir::types::I8)', 'Same', 'Same', 'Concrete(ir::types::I8)']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x6a, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Band: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 10, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Bor: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 10, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Bxor: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 10, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Bnot: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 10, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Rotl: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Free(0)']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 46, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Rotr: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Free(0)']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 46, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Ishl: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Free(0)']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 46, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Ushr: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Free(0)']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 46, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Sshr: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Free(0)']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 46, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Bitrev: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Clz: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Cls: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Ctz: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Bswap: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 13, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Popcnt: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 11, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fcmp: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=2
    // Constraints=['AsTruthy', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x59, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fadd: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fsub: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fmul: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fdiv: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Sqrt: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fma: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=3
    // Constraints=['Same', 'Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x69, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 18, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fneg: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fabs: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fcopysign: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fmin: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fmax: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Ceil: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Floor: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Trunc: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Nearest: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Same']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x29, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 14, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Bitcast: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(5)']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 5, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // ScalarToVector: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'LaneOf']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 9, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 4, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Bmask: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(0)']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 32, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Ireduce: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Wider']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 51, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Snarrow: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=2
    // Constraints=['SplitLanes', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8}, ints={16, 32, 64})
    OpcodeConstraints {
        flags: 0x59, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 15, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 53, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Unarrow: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=2
    // Constraints=['SplitLanes', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8}, ints={16, 32, 64})
    OpcodeConstraints {
        flags: 0x59, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 15, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 53, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Uunarrow: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=2
    // Constraints=['SplitLanes', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8}, ints={16, 32, 64})
    OpcodeConstraints {
        flags: 0x59, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 15, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 53, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SwidenLow: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['MergeLanes', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16}, ints={8, 16, 32})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 56, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SwidenHigh: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['MergeLanes', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16}, ints={8, 16, 32})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 56, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // UwidenLow: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['MergeLanes', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16}, ints={8, 16, 32})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 56, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // UwidenHigh: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['MergeLanes', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16}, ints={8, 16, 32})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 56, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // IaddPairwise: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={2, 4, 8, 16}, ints={8, 16, 32})
    OpcodeConstraints {
        flags: 0x49, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 16, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // X86Pmaddubsw: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Concrete(ir::types::I16X8)', 'Concrete(ir::types::I8X16)', 'Concrete(ir::types::I8X16)']
    OpcodeConstraints {
        flags: 0x41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 58, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Uextend: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Narrower']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 61, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Sextend: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Narrower']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 61, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fpromote: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Narrower']
    // Polymorphic over TypeSet(lanes={1}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 17, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 61, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fdemote: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Wider']
    // Polymorphic over TypeSet(lanes={1}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 17, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 51, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fvdemote: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Concrete(ir::types::F32X4)', 'Concrete(ir::types::F64X2)']
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 63, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // FvpromoteLow: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Concrete(ir::types::F64X2)', 'Concrete(ir::types::F32X4)']
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 64, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // FcvtToUint: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(17)']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 66, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // FcvtToSint: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(17)']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 66, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // FcvtToUintSat: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(14)']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 3, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 68, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // FcvtToSintSat: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(14)']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 3, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 68, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // X86Cvtt2dq: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(14)']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 3, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 68, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // FcvtFromUint: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(3)']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 18, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 70, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // FcvtFromSint: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(3)']
    // Polymorphic over TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 18, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 70, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Isplit: fixed_results=2, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['HalfWidth', 'HalfWidth', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x3a, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 13, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 72, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Iconcat: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=2
    // Constraints=['DoubleWidth', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64})
    OpcodeConstraints {
        flags: 0x59, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 8, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 75, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // AtomicRmw: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=2
    // Constraints=['Same', 'Free(1)', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x41, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 77, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // AtomicCas: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=false, fixed_values=3
    // Constraints=['Same', 'Free(1)', 'Same', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x69, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 77, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // AtomicLoad: fixed_results=1, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=1
    // Constraints=['Same', 'Free(1)']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x21, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 12, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // AtomicStore: fixed_results=0, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=2
    // Constraints=['Same', 'Free(1)']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x58, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 12, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // Fence: fixed_results=0, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=[]
    OpcodeConstraints {
        flags: 0x00, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // ExtractVector: fixed_results=1, use_typevar_operand=true, requires_typevar_operand=true, fixed_values=1
    // Constraints=['DynamicToVector', 'Same']
    // Polymorphic over TypeSet(lanes={1}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
    OpcodeConstraints {
        flags: 0x39, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 19, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 81, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
    // SequencePoint: fixed_results=0, use_typevar_operand=false, requires_typevar_operand=false, fixed_values=0
    // Constraints=[]
    OpcodeConstraints {
        flags: 0x00, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:969
        typeset_offset: 255, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:970
        constraint_offset: 0, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:971
    }
    ,
]; // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:976

// Table of value type sets.
const TYPE_SETS: [ir::instructions::ValueTypeSet; 20] = [ // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:859
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1}, ints={8, 16, 32, 64, 128})
        lanes: ScalarBitSet::<u16>(1), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(248), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1}, ints={32, 64})
        lanes: ScalarBitSet::<u16>(1), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(96), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
        lanes: ScalarBitSet::<u16>(510), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(510), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(248), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(240), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
        lanes: ScalarBitSet::<u16>(511), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(248), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
        lanes: ScalarBitSet::<u16>(510), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(248), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
        lanes: ScalarBitSet::<u16>(511), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(510), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(248), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(240), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1}, ints={16, 32, 64})
        lanes: ScalarBitSet::<u16>(1), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(112), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1}, ints={64})
        lanes: ScalarBitSet::<u16>(1), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(64), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1}, ints={8, 16, 32, 64})
        lanes: ScalarBitSet::<u16>(1), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(120), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
        lanes: ScalarBitSet::<u16>(510), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(248), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(240), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
        lanes: ScalarBitSet::<u16>(511), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(248), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(240), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, ints={8, 16, 32, 64, 128})
        lanes: ScalarBitSet::<u16>(511), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(510), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(248), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={4, 8}, ints={16, 32})
        lanes: ScalarBitSet::<u16>(12), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(48), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1}, ints={16, 32, 64, 128})
        lanes: ScalarBitSet::<u16>(1), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(240), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
        lanes: ScalarBitSet::<u16>(511), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(510), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(240), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={2, 4, 8}, ints={16, 32, 64})
        lanes: ScalarBitSet::<u16>(14), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(14), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(112), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={2, 4, 8, 16}, ints={8, 16, 32})
        lanes: ScalarBitSet::<u16>(30), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(30), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(56), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1}, floats={16, 32, 64, 128})
        lanes: ScalarBitSet::<u16>(1), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(240), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1, 2, 4, 8, 16, 32, 64, 128, 256}, floats={16, 32, 64, 128})
        lanes: ScalarBitSet::<u16>(511), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(240), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
    ir::instructions::ValueTypeSet {
        // TypeSet(lanes={1}, ints={8, 16, 32, 64, 128}, floats={16, 32, 64, 128})
        lanes: ScalarBitSet::<u16>(1), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        dynamic_lanes: ScalarBitSet::<u16>(510), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        ints: ScalarBitSet::<u8>(248), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
        floats: ScalarBitSet::<u8>(240), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:827
    }
    ,
]; // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:876

// Table of operand constraint sequences.
const OPERAND_CONSTRAINTS: [OperandConstraint; 83] = [ // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:983
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I32), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::LaneOf, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I8X16), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I8X16), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I8X16), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::LaneOf, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Free(1), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I64), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I64), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Free(1), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I16X8), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I32X4), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I64X2), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::F16), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::F32), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::F64), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::F128), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Free(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I8), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Free(9), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::AsTruthy, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I8), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I8), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Free(0), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Free(5), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Wider, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::SplitLanes, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::MergeLanes, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I16X8), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I8X16), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::I8X16), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Narrower, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::F32X4), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::F64X2), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Concrete(ir::types::F32X4), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Free(17), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Free(14), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Free(3), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::HalfWidth, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::HalfWidth, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::DoubleWidth, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Free(1), // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::DynamicToVector, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
    OperandConstraint::Same, // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:990
]; // /Users/skylerberg/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cranelift-codegen-meta-0.134.3/src/gen_inst.rs:993
