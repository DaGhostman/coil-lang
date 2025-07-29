#[repr(u8)]
#[derive(Debug, Default, Copy, Clone)]
pub enum Bytecode {
    ADD = 0,
    SUB,
    MUL,
    DIV,
    INC,
    DEC,
    ADDF,
    SUBF,
    MULF,
    DIVF,
    EQ = 30,
    LE,
    GT,
    JMP = 50,
    JMPT,
    JMPF,
    CALL,
    PRINT = 100,
    RETURN,
    PUSH = 200,
    POP,
    LOAD,
    STORE,
    MOVE,
    CONST,
    SUSP = 251,
    DATA = 253,
    #[default]
    HALT = 254,
}

impl From<u8> for Bytecode {
    fn from(value: u8) -> Self {
        match value {
            0 => Bytecode::ADD,
            1 => Bytecode::SUB,
            2 => Bytecode::MUL,
            3 => Bytecode::DIV,
            4 => Bytecode::ADDF,
            5 => Bytecode::SUBF,
            6 => Bytecode::MULF,
            7 => Bytecode::DIVF,
            30 => Bytecode::EQ,
            31 => Bytecode::LE,
            32 => Bytecode::GT,
            50 => Bytecode::JMP,
            51 => Bytecode::JMPT,
            52 => Bytecode::JMPF,
            53 => Bytecode::CALL,
            100 => Bytecode::PRINT,
            101 => Bytecode::RETURN,
            200 => Bytecode::PUSH,
            201 => Bytecode::POP,
            202 => Bytecode::LOAD,
            203 => Bytecode::STORE,
            204 => Bytecode::CONST,
            251 => Bytecode::SUSP,
            254 => Bytecode::HALT,
            _ => Bytecode::HALT,
        }
    }
}

impl Into<u8> for Bytecode {
    fn into(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Opcode {
    bytecode: Bytecode,
    operands: [u8; 3],
}

impl Opcode {
    pub fn new(bytecode: Bytecode, operands: [u8; 3]) -> Self {
        Self {
            bytecode,
            operands,
        }
    }
    
    pub fn bytecode(&self) -> Bytecode {
        self.bytecode
    }

    pub fn operand(&self, idx: usize) -> u8 {
        self.operands[idx]
    }

    pub fn constant<T: From<u32>>(&self) -> T {
        u32::from_be_bytes([
            0,
            self.operands[0],
            self.operands[1],
            self.operands[2],
        ]).into() 
    }
}

impl Into<u32> for Opcode {
    fn into(self) -> u32 {
        let bytes = [
            (self.bytecode as u8).to_be_bytes()[0],
            self.operands[0],
            self.operands[1],
            self.operands[2],
        ];

        u32::from_be_bytes(bytes)
    }
}

impl From<u32> for Opcode {
    fn from(value: u32) -> Self {
        let opcode = (value >> 24) & 254;

        let operands = [
            ((value & 0x00FF0000) >> 6) as u8,
            ((value & 0x0000FF00) >> 4) as u8,
            (value & 0x000000FF) as u8,
        ];

        Self {
            bytecode: Bytecode::from(opcode as u8),
            operands,
        }
    }
}
