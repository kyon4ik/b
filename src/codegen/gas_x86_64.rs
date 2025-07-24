use crate::crust::libc::*;
use crate::ir::*;
use crate::lexer::Loc;
use crate::nob::*;
use crate::targets::Os;
use core::cmp;
use core::ffi::*;
use core::mem::*;

const PTR_WIDTH: usize = 8;
const FUNC_ALIGNMENT: usize = 16;

pub unsafe fn align_bytes(bytes: usize, alignment: usize) -> usize {
    bytes.next_multiple_of(alignment)
}

macro_rules! reg_type {
    ($name:ident { $($reg_name:ident),+ }) => {
        #[derive(Clone, Copy)]
        pub enum $name {
            $($reg_name),+
        }

        impl $name {
            pub unsafe fn as_str(self: $name) -> *const c_char {
                match self {
                    $(Self::$reg_name => c!(stringify!($reg_name))),+
                }
            }
        }
    }
}

macro_rules! emit_unop {
    ($builder:expr, $name:literal, $reg:expr) => {{
        write($builder, c!(concat!($name, " ")));
        emit_reg($builder, $reg);
    }};
}

macro_rules! emit_binop {
    ($builder:expr, $name:literal, $r1:expr, $r2:expr) => {{
        write($builder, c!(concat!($name, " ")));
        emit_reg_or_const($builder, $r1);
        sb_appendf((*$builder).output, c!(", "));
        emit_reg($builder, $r2);
    }};
}

reg_type!(Reg {
    rax,
    rbx,
    rcx,
    rdx,
    rsi,
    rdi,
    rbp,
    rsp,
    r8,
    r9,
    r10,
    r11,
    r12,
    r13,
    r14,
    r15
});

reg_type!(Reg8 {
    al,
    bl,
    cl,
    dl,
    sil,
    bpl,
    spl,
    r8b,
    r9b,
    r10b,
    r11b,
    r12b,
    r13b,
    r14b,
    r15b
});

#[derive(Clone, Copy)]
pub enum RegOrConst {
    Reg(Reg),
    Const(i64),
}

#[derive(Clone, Copy)]
pub enum Mem {
    Reg(Reg),
    RegOffset(Reg, i64),
    RegExpr(Reg, Reg, usize),
    Relative(*const c_char),
    SectionRel(*const c_char, usize),
}

#[derive(Clone, Copy)]
pub struct AsmEmitter {
    output: *mut String_Builder,
    os: Os,
}

pub unsafe fn write(b: *mut AsmEmitter, bytes: *const c_char) {
    sb_appendf((*b).output, bytes);
}

pub unsafe fn emit_section(b: *mut AsmEmitter, section_name: *const c_char) {
    let fmt = match (*b).os {
        Os::Linux | Os::Windows => c!(".section .%s\n"),
        Os::Darwin => c!(".%s\n"),
    };
    sb_appendf((*b).output, fmt, section_name);
}

pub unsafe fn emit_symbol(b: *mut AsmEmitter, name: *const c_char) {
    match (*b).os {
        Os::Linux | Os::Windows => sb_appendf((*b).output, c!("%s"), name),
        Os::Darwin => sb_appendf((*b).output, c!("_%s"), name),
    };
}

pub unsafe fn emit_global(b: *mut AsmEmitter, name: *const c_char, alignment: usize) {
    write(b, c!(".global "));
    emit_symbol(b, name);
    write(b, c!("\n"));

    assert!(alignment.is_power_of_two());
    sb_appendf((*b).output, c!(".p2align %zu\n"), alignment.ilog2());
    emit_symbol(b, name);
    write(b, c!(":\n"));
}

pub unsafe fn emit_asm_line(b: *mut AsmEmitter, line: *const c_char) {
    sb_appendf((*b).output, c!("%s\n"), line);
}

pub unsafe fn emit_label(b: *mut AsmEmitter, func_name: *const c_char, label: usize) {
    match (*b).os {
        Os::Linux | Os::Windows => sb_appendf((*b).output, c!(".L%s_label_%zu"), func_name, label),
        Os::Darwin => sb_appendf((*b).output, c!("L%s_label_%zu"), func_name, label),
    };
}

pub unsafe fn emit_reg(b: *mut AsmEmitter, reg: Reg) {
    sb_appendf((*b).output, c!("%%%s"), reg.as_str());
}

pub unsafe fn emit_reg8(b: *mut AsmEmitter, reg: Reg8) {
    sb_appendf((*b).output, c!("%%%s"), reg.as_str());
}

pub unsafe fn emit_mem(b: *mut AsmEmitter, mem: Mem) {
    match mem {
        Mem::Reg(reg) => {
            emit_reg(b, reg);
        }
        Mem::RegOffset(reg, offset) => {
            emit_const(b, -offset, IntBase::Hex);
            write(b, c!("("));
            emit_reg(b, reg);
            write(b, c!(")"));
        }
        Mem::RegExpr(base, offset, step) => {
            write(b, c!("("));
            emit_reg(b, base);
            write(b, c!(","));
            emit_reg(b, offset);
            write(b, c!(","));
            sb_appendf((*b).output, c!("%zu"), step);
            write(b, c!(")"));
        }
        Mem::Relative(symbol) => {
            emit_symbol(b, symbol);
            if (*b).os == Os::Darwin {
                write(b, c!("@GOTPCREL"));
            }
            write(b, c!("(%%rip)"));
        }
        Mem::SectionRel(section, offset) => {
            write(b, section);
            sb_appendf((*b).output, c!("+%zu(%%rip)"), offset);
        }
    }
}

#[derive(Clone, Copy)]
pub enum IntBase {
    Dec,
    Hex,
} 

pub unsafe fn emit_const(b: *mut AsmEmitter, value: i64, base: IntBase) {
    match base {
        IntBase::Dec => sb_appendf((*b).output, c!("%lld"), value),
        IntBase::Hex => {
            let abs = value.unsigned_abs();
            let sign = if value < 0 { c!("-") } else { c!("") };
            sb_appendf((*b).output, c!("%s0x%llX"), sign, abs) 
        }
    };
}

pub unsafe fn emit_reg_or_const(b: *mut AsmEmitter, reg_or_const: RegOrConst) {
    match reg_or_const {
        RegOrConst::Reg(reg) => emit_reg(b, reg),
        RegOrConst::Const(value) => {
            write(b, c!("$"));
            emit_const(b, value, IntBase::Dec)
        },
    }
}

pub unsafe fn emit_instr(b: *mut AsmEmitter, instr: Instr) {
    match instr {
        Instr::Call(symbol) => {
            write(b, c!("call "));
            emit_symbol(b, symbol);
        }
        Instr::CallIndirect(reg) => {
            write(b, c!("call *"));
            emit_reg(b, reg);
        }
        Instr::Store(reg_or_const, mem) => {
            write(b, c!("movq "));
            emit_reg_or_const(b, reg_or_const);
            sb_appendf((*b).output, c!(", "));
            emit_mem(b, mem);
        }
        Instr::Load(mem, reg) => {
            write(b, c!("movq "));
            emit_mem(b, mem);
            sb_appendf((*b).output, c!(", "));
            emit_reg(b, reg);
        }
        Instr::Leaq(mem, reg) => {
            write(b, c!("leaq "));
            emit_mem(b, mem);
            sb_appendf((*b).output, c!(", "));
            emit_reg(b, reg);
        }
        Instr::Set(cc, reg) => {
            let name = match cc {
                CondCode::Zero => c!("setz"),
                CondCode::Eq => c!("sete"),
                CondCode::Neq => c!("setne"),
                CondCode::Lt => c!("setl"),
                CondCode::Gt => c!("setg"),
                CondCode::Lte => c!("setle"),
                CondCode::Gte => c!("setge"),
            };
            write(b, name);
            write(b, c!(" "));
            emit_reg8(b, reg);
        }
        Instr::Pushq(reg) => emit_unop!(b, "pushq", reg),
        Instr::Popq(reg) => emit_unop!(b, "popq", reg),
        Instr::Negq(reg) => emit_unop!(b, "negq", reg),
        Instr::IDivq(reg) => emit_unop!(b, "idivq", reg),
        Instr::Addq(rc, rg) => emit_binop!(b, "addq", rc, rg),
        Instr::Subq(rc, rg) => emit_binop!(b, "subq", rc, rg),
        Instr::IMulq(rc, rg) => emit_binop!(b, "imulq", rc, rg),
        Instr::Andq(rc, rg) => emit_binop!(b, "andq", rc, rg),
        Instr::Orq(rc, rg) => emit_binop!(b, "orq", rc, rg),
        Instr::Xorq(rc, rg) => emit_binop!(b, "xorq", rc, rg),
        Instr::Cmpq(rc, rg) => emit_binop!(b, "cmpq", rc, rg),
        Instr::Testq(rc, rg) => emit_binop!(b, "testq", rc, rg),
        Instr::Shlq(shift, reg) => {
            write(b, c!("shlq "));
            emit_reg8(b, shift);
            write(b, c!(", "));
            emit_reg(b, reg);
        }
        Instr::Shrq(shift, reg) => {
            write(b, c!("shrq "));
            emit_reg8(b, shift);
            write(b, c!(", "));
            emit_reg(b, reg);
        }
        Instr::Jmp(func_name, label) => {
            write(b, c!("jmp "));
            emit_label(b, func_name, label);
        }
        Instr::CondJmp(cc, func_name, label) => {
            let name = match cc {
                CondCode::Zero => c!("jz"),
                CondCode::Eq => c!("je"),
                CondCode::Neq => c!("jne"),
                CondCode::Lt => c!("jl"),
                CondCode::Gt => c!("jg"),
                CondCode::Lte => c!("jle"),
                CondCode::Gte => c!("jge"),
            };
            write(b, name);
            write(b, c!(" "));
            emit_label(b, func_name, label);
        }
        Instr::Cqto => {
            write(b, c!("cqto"));
        }
        Instr::Ret => {
            write(b, c!("ret"));
        }
    }
    write(b, c!("\n"));
}

#[derive(Clone, Copy)]
pub enum CondCode {
    Zero,
    Eq,
    Neq,
    Lt,
    Gt,
    Lte,
    Gte,
}

#[derive(Clone, Copy)]
pub enum Instr {
    Call(*const c_char),
    CallIndirect(Reg),      // call
    Load(Mem, Reg),         // movq
    Store(RegOrConst, Mem), // movq
    Leaq(Mem, Reg),
    Set(CondCode, Reg8),
    Pushq(Reg),
    Popq(Reg),
    Negq(Reg),
    IDivq(Reg),
    Addq(RegOrConst, Reg),
    Subq(RegOrConst, Reg),
    IMulq(RegOrConst, Reg),
    Andq(RegOrConst, Reg),
    Orq(RegOrConst, Reg),
    Xorq(RegOrConst, Reg),
    Cmpq(RegOrConst, Reg),
    Testq(RegOrConst, Reg),
    Shlq(Reg8, Reg),
    Shrq(Reg8, Reg),
    Jmp(*const c_char, usize),
    CondJmp(CondCode, *const c_char, usize),
    Cqto,
    Ret,
}

pub unsafe fn call_arg(b: *mut AsmEmitter, arg: Arg) {
    match arg {
        Arg::RefExternal(name) | Arg::External(name) => {
            emit_instr(b, Instr::Call(name));
        }
        arg => {
            load_arg_to_reg(b, arg, Reg::rax);
            emit_instr(b, Instr::CallIndirect(Reg::rax));
        }
    };
}

pub unsafe fn load_arg_to_reg(b: *mut AsmEmitter, arg: Arg, reg: Reg) {
    let instr = match arg {
        Arg::Deref(index) => {
            emit_instr(
                b,
                Instr::Load(Mem::RegOffset(Reg::rbp, (index * PTR_WIDTH) as i64), reg),
            );
            Instr::Load(Mem::RegOffset(reg, 0), reg)
        }
        Arg::RefAutoVar(index) => {
            Instr::Leaq(Mem::RegOffset(Reg::rbp, (index * PTR_WIDTH) as i64), reg)
        }
        Arg::RefExternal(name) => Instr::Leaq(Mem::Relative(name), reg),
        Arg::External(name) => Instr::Load(Mem::Relative(name), reg),
        Arg::AutoVar(index) => {
            Instr::Load(Mem::RegOffset(Reg::rbp, (index * PTR_WIDTH) as i64), reg)
        }
        Arg::Literal(value) => Instr::Store(RegOrConst::Const(value as i64), Mem::Reg(reg)),
        Arg::DataOffset(offset) => Instr::Leaq(Mem::SectionRel(c!("dat"), offset), reg),
        Arg::Bogus => unreachable!("bogus-amogus"),
    };
    emit_instr(b, instr);
}

pub unsafe fn emit_function(
    b: *mut AsmEmitter,
    func: *const Func,
    func_index: usize,
    debug: bool,
) {
    let stack_size = align_bytes((*func).auto_vars_count * PTR_WIDTH, FUNC_ALIGNMENT);
    emit_global(b, (*func).name, FUNC_ALIGNMENT);

    // if debug {
    //     sb_appendf(output, c!("    .file %lld \"%s\"\n"), func_index, name_loc.input_path);
    //     // we need to place line information directly after the label, before any instrucitons
    //     // ideally pointing to the first statement instead of the function name
    //     if body.len() > 0 {
    //         sb_appendf(output, c!("    .loc %lld %lld\n"), func_index, (*body)[0].loc.line_number);
    //     } else {
    //         sb_appendf(output, c!("    .loc %lld %lld\n"), func_index, name_loc.line_number);
    //     }
    // }

    emit_instr(b, Instr::Pushq(Reg::rbp));
    emit_instr(
        b,
        Instr::Store(RegOrConst::Reg(Reg::rsp), Mem::Reg(Reg::rbp)),
    );
    if stack_size > 0 {
        emit_instr(
            b,
            Instr::Subq(RegOrConst::Const(stack_size as i64), Reg::rsp),
        );
    }

    assert!((*func).auto_vars_count >= (*func).params_count);

    let param_regs: *const [Reg] = match (*b).os {
        Os::Linux | Os::Darwin => &[Reg::rdi, Reg::rsi, Reg::rdx, Reg::rcx, Reg::r8, Reg::r9],
        // https://en.wikipedia.org/wiki/X86_calling_conventions#Microsoft_x64_calling_convention
        Os::Windows => &[Reg::rcx, Reg::rdx, Reg::r8, Reg::r9],
    };

    let non_reg_params_offset = match (*b).os {
        Os::Linux | Os::Darwin => 2,
        Os::Windows => 6,
    };

    for i in 0..(*func).params_count {
        let auto_offset = (i + 1) * PTR_WIDTH;
        if i < param_regs.len() {
            let reg = (*param_regs)[i];
            emit_instr(
                b,
                Instr::Store(
                    RegOrConst::Reg(reg),
                    Mem::RegOffset(Reg::rbp, auto_offset as i64),
                ),
            );
        } else {
            let param_offset = (i - param_regs.len() + non_reg_params_offset) * PTR_WIDTH;
            emit_instr(
                b,
                Instr::Load(Mem::RegOffset(Reg::rbp, -(param_offset as i64)), Reg::rax),
            );
            emit_instr(
                b,
                Instr::Store(
                    RegOrConst::Reg(Reg::rax),
                    Mem::RegOffset(Reg::rbp, auto_offset as i64),
                ),
            );
        }
    }

    let body = da_slice((*func).body);
    for i in 0..body.len() {
        let op = (*body)[i];

        // if debug {
        //     // location info of the first op has already been pushed
        //     if i > 0 {
        //         sb_appendf(output, c!("    .loc %lld %lld\n"), func_index, op.loc.line_number);
        //     }
        // }

        match op.opcode {
            Op::Bogus => unreachable!("bogus-amogus"),
            Op::Return { arg } => {
                if let Some(arg) = arg {
                    load_arg_to_reg(b, arg, Reg::rax);
                }
                emit_instr(
                    b,
                    Instr::Store(RegOrConst::Reg(Reg::rbp), Mem::Reg(Reg::rsp)),
                );
                emit_instr(b, Instr::Popq(Reg::rbp));
                emit_instr(b, Instr::Ret);
            }
            Op::Store { index, arg } => {
                emit_instr(
                    b,
                    Instr::Load(
                        Mem::RegOffset(Reg::rbp, (index * PTR_WIDTH) as i64),
                        Reg::rax,
                    ),
                );
                load_arg_to_reg(b, arg, Reg::rcx);
                emit_instr(
                    b,
                    Instr::Store(RegOrConst::Reg(Reg::rcx), Mem::RegOffset(Reg::rax, 0)),
                );
            }
            Op::ExternalAssign { name, arg } => {
                load_arg_to_reg(b, arg, Reg::rax);
                emit_instr(
                    b,
                    Instr::Store(RegOrConst::Reg(Reg::rax), Mem::Relative(name)),
                );
            }
            Op::AutoAssign { index, arg } => {
                load_arg_to_reg(b, arg, Reg::rax);
                emit_instr(
                    b,
                    Instr::Store(
                        RegOrConst::Reg(Reg::rax),
                        Mem::RegOffset(Reg::rbp, (index * PTR_WIDTH) as i64),
                    ),
                );
            }
            Op::Negate { result, arg } => {
                load_arg_to_reg(b, arg, Reg::rax);
                emit_instr(b, Instr::Negq(Reg::rax));
                emit_instr(
                    b,
                    Instr::Store(
                        RegOrConst::Reg(Reg::rax),
                        Mem::RegOffset(Reg::rbp, (result * PTR_WIDTH) as i64),
                    ),
                );
            }
            Op::UnaryNot { result, arg } => {
                emit_instr(b, Instr::Xorq(RegOrConst::Reg(Reg::rcx), Reg::rcx));
                load_arg_to_reg(b, arg, Reg::rax);
                emit_instr(b, Instr::Testq(RegOrConst::Reg(Reg::rax), Reg::rax));
                emit_instr(b, Instr::Set(CondCode::Zero, Reg8::cl));
                emit_instr(
                    b,
                    Instr::Store(
                        RegOrConst::Reg(Reg::rcx),
                        Mem::RegOffset(Reg::rbp, (result * PTR_WIDTH) as i64),
                    ),
                );
            }
            Op::Binop {
                binop,
                index,
                lhs,
                rhs,
            } => {
                load_arg_to_reg(b, lhs, Reg::rax);
                load_arg_to_reg(b, rhs, Reg::rcx);
                match binop {
                    Binop::BitOr => {
                        emit_instr(b, Instr::Orq(RegOrConst::Reg(Reg::rcx), Reg::rax));
                    }
                    Binop::BitAnd => {
                        emit_instr(b, Instr::Andq(RegOrConst::Reg(Reg::rcx), Reg::rax));
                    }
                    Binop::BitShl => {
                        load_arg_to_reg(b, rhs, Reg::rcx);
                        emit_instr(b, Instr::Shlq(Reg8::cl, Reg::rax));
                    }
                    Binop::BitShr => {
                        load_arg_to_reg(b, rhs, Reg::rcx);
                        emit_instr(b, Instr::Shrq(Reg8::cl, Reg::rax));
                    }
                    Binop::Plus => {
                        emit_instr(b, Instr::Addq(RegOrConst::Reg(Reg::rcx), Reg::rax));
                    }
                    Binop::Minus => {
                        emit_instr(b, Instr::Subq(RegOrConst::Reg(Reg::rcx), Reg::rax));
                    }
                    Binop::Mult => {
                        emit_instr(b, Instr::IMulq(RegOrConst::Reg(Reg::rcx), Reg::rax));
                    }
                    Binop::Div => {
                        emit_instr(b, Instr::Cqto);
                        emit_instr(b, Instr::IDivq(Reg::rcx));
                    }
                    Binop::Mod => {
                        emit_instr(b, Instr::Cqto);
                        emit_instr(b, Instr::IDivq(Reg::rcx));
                        emit_instr(
                            b,
                            Instr::Store(
                                RegOrConst::Reg(Reg::rdx),
                                Mem::RegOffset(Reg::rbp, (index * PTR_WIDTH) as i64),
                            ),
                        );
                        continue;
                    }
                    _ => {
                        emit_instr(b, Instr::Xorq(RegOrConst::Reg(Reg::rdx), Reg::rdx));
                        emit_instr(b, Instr::Cmpq(RegOrConst::Reg(Reg::rcx), Reg::rax));
                        let cc = match binop {
                            Binop::Less => CondCode::Lt,
                            Binop::Greater => CondCode::Gt,
                            Binop::Equal => CondCode::Eq,
                            Binop::NotEqual => CondCode::Neq,
                            Binop::GreaterEqual => CondCode::Gte,
                            Binop::LessEqual => CondCode::Lte,
                            _ => unreachable!(),
                        };
                        emit_instr(b, Instr::Set(cc, Reg8::dl));
                        emit_instr(
                            b,
                            Instr::Store(
                                RegOrConst::Reg(Reg::rdx),
                                Mem::RegOffset(Reg::rbp, (index * PTR_WIDTH) as i64),
                            ),
                        );
                        continue;
                    }
                }
                emit_instr(
                    b,
                    Instr::Store(
                        RegOrConst::Reg(Reg::rax),
                        Mem::RegOffset(Reg::rbp, (index * PTR_WIDTH) as i64),
                    ),
                );
            }
            Op::Funcall { result, fun, args } => {
                let reg_args_count = cmp::min(args.count, param_regs.len());
                for i in 0..reg_args_count {
                    let reg = (*param_regs)[i];
                    load_arg_to_reg(b, *args.items.add(i), reg);
                }

                let stack_args_count = args.count - reg_args_count;
                let stack_args_size = align_bytes(stack_args_count * PTR_WIDTH, FUNC_ALIGNMENT);
                if stack_args_count > 0 {
                    emit_instr(
                        b,
                        Instr::Subq(RegOrConst::Const(stack_args_size as i64), Reg::rsp),
                    );
                    for i in 0..stack_args_count {
                        load_arg_to_reg(b, *args.items.add(reg_args_count + i), Reg::rax);
                        emit_instr(
                            b,
                            Instr::Store(
                                RegOrConst::Reg(Reg::rax),
                                Mem::RegOffset(Reg::rsp, -((i * PTR_WIDTH) as i64)),
                            ),
                        );
                    }
                }

                match (*b).os {
                    Os::Linux | Os::Darwin => {
                        // x86_64 Linux ABI passes the amount of
                        // floating point args via al. Since B
                        // does not distinguish regular and
                        // variadic functions we set al to 0 just in case.

                        // FIXME: provide movb instruction
                        sb_appendf((*b).output, c!("movb $0, %%al\n"));
                        call_arg(b, fun);
                    }
                    Os::Windows => {
                        // allocate 32 bytes for "shadow space"
                        // it must be allocated at the top of the stack after all arguments are pushed
                        // so we can't allocate it at function prologue
                        emit_instr(b, Instr::Subq(RegOrConst::Const(32), Reg::rsp));
                        call_arg(b, fun);
                        emit_instr(b, Instr::Addq(RegOrConst::Const(32), Reg::rsp));
                    }
                }
                if stack_args_count > 0 {
                    emit_instr(
                        b,
                        Instr::Addq(RegOrConst::Const(stack_args_size as i64), Reg::rsp),
                    );
                }
                emit_instr(
                    b,
                    Instr::Store(
                        RegOrConst::Reg(Reg::rax),
                        Mem::RegOffset(Reg::rbp, (result * PTR_WIDTH) as i64),
                    ),
                );
            }
            Op::Asm { stmts } => {
                for i in 0..stmts.count {
                    let stmt = *stmts.items.add(i);
                    emit_asm_line(b, stmt.line);
                }
            }
            //All labels are global in GAS, so we need to namespace them.
            Op::Label { label } => {
                emit_label(b, (*func).name, label);
                write(b, c!(":\n"));
            }
            Op::JmpLabel { label } => {
                emit_instr(b, Instr::Jmp((*func).name, label));
            }
            Op::JmpIfNotLabel { label, arg } => {
                load_arg_to_reg(b, arg, Reg::rax);
                emit_instr(b, Instr::Testq(RegOrConst::Reg(Reg::rax), Reg::rax));
                emit_instr(b, Instr::CondJmp(CondCode::Zero, (*func).name, label));
            }
            Op::Index {
                result,
                arg,
                offset,
            } => {
                load_arg_to_reg(b, arg, Reg::rax);
                load_arg_to_reg(b, offset, Reg::rcx);
                emit_instr(
                    b,
                    Instr::Leaq(Mem::RegExpr(Reg::rax, Reg::rcx, PTR_WIDTH), Reg::rax),
                );
                emit_instr(
                    b,
                    Instr::Store(
                        RegOrConst::Reg(Reg::rax),
                        Mem::RegOffset(Reg::rbp, (result * PTR_WIDTH) as i64),
                    ),
                );
            }
        }
    }
    emit_instr(b, Instr::Store(RegOrConst::Const(0), Mem::Reg(Reg::rax)));
    emit_instr(
        b,
        Instr::Store(RegOrConst::Reg(Reg::rbp), Mem::Reg(Reg::rsp)),
    );
    emit_instr(b, Instr::Popq(Reg::rbp));
    emit_instr(b, Instr::Ret);
}

pub unsafe fn emit_funcs(b: *mut AsmEmitter, funcs: *const [Func], debug: bool) {
    for i in 0..funcs.len() {
        let func = &(*funcs)[i];
        emit_function(b, func, i, debug);
    }
}

pub unsafe fn emit_asm_funcs(asm: *mut AsmEmitter, asm_funcs: *const [AsmFunc]) {
    for i in 0..asm_funcs.len() {
        let asm_func = (*asm_funcs)[i];
        emit_global(asm, asm_func.name, FUNC_ALIGNMENT);

        for j in 0..asm_func.body.count {
            let stmt = *asm_func.body.items.add(j);
            emit_asm_line(asm, stmt.line);
        }
    }
}

pub unsafe fn emit_globals(asm: *mut AsmEmitter, globals: *const [Global]) {
    for i in 0..globals.len() {
        let global = (*globals)[i];
        emit_global(asm, global.name, PTR_WIDTH);

        if global.is_vec {
            sb_appendf((*asm).output, c!(".quad . + %zu\n"), PTR_WIDTH);
        }

        if global.values.count > 0 {
            write(asm, c!(".quad "));
            for j in 0..global.values.count {
                if j > 0 {
                    write(asm, c!(","));
                }
                match *global.values.items.add(j) {
                    ImmediateValue::Literal(lit) => emit_const(asm, lit as i64, IntBase::Hex),
                    ImmediateValue::Name(name) => emit_symbol(asm, name),
                    ImmediateValue::DataOffset(offset) => { sb_appendf((*asm).output, c!("dat+%zu"), offset); }
                };
            }
            write(asm, c!("\n"));
        }
        if global.values.count < global.minimum_size {
            let reserved_qwords = global.minimum_size - global.values.count;
            if reserved_qwords > 0 {
                sb_appendf((*asm).output, c!(".space %zu\n"), reserved_qwords * PTR_WIDTH);
            }
        }
    }
}

pub unsafe fn emit_data(asm: *mut AsmEmitter, data: *const [u8]) {
    if data.len() > 0 {
        write(asm, c!("dat: .byte "));
        for i in 0..data.len() {
            if i > 0 {
                write(asm, c!(","));
            }
            emit_const(asm, (*data)[i] as i64, IntBase::Hex);
        }
        write(asm, c!("\n"));
    }
}

pub unsafe fn generate_program(
    // Inputs
    p: *const Program,
    program_path: *const c_char,
    garbage_base: *const c_char,
    linker: *const [*const c_char],
    os: Os,
    nostdlib: bool,
    debug: bool,
    // Temporaries
    output: *mut String_Builder,
    cmd: *mut Cmd,
) -> Option<()> {
    let mut asm_emitter = AsmEmitter { output, os };

    emit_section(&mut asm_emitter, c!("text"));
    emit_funcs(&mut asm_emitter, da_slice((*p).funcs), debug);
    emit_asm_funcs(&mut asm_emitter, da_slice((*p).asm_funcs));
    
    emit_section(&mut asm_emitter, c!("data"));
    emit_data(&mut asm_emitter, da_slice((*p).data));
    emit_globals(&mut asm_emitter, da_slice((*p).globals));

    let output_asm_path = temp_sprintf(c!("%s.s"), garbage_base);
    write_entire_file(
        output_asm_path,
        (*output).items as *const c_void,
        (*output).count,
    )?;
    log(Log_Level::INFO, c!("generated %s"), output_asm_path);

    match os {
        Os::Darwin => {
            if !(cfg!(target_os = "macos")) {
                // TODO: think how to approach cross-compilation
                log(
                    Log_Level::ERROR,
                    c!("Cross-compilation of darwin is not supported"),
                );
                return None;
            }

            let (gas, cc) = (c!("as"), c!("cc"));

            let output_obj_path = temp_sprintf(c!("%s.o"), program_path);
            cmd_append! {
                cmd,
                gas, c!("-arch"), c!("x86_64"), c!("-o"), output_obj_path, output_asm_path,
            }
            if !cmd_run_sync_and_reset(cmd) {
                return None;
            }

            cmd_append! {
                cmd,
                cc, c!("-arch"), c!("x86_64"), c!("-o"), program_path, output_obj_path,
            }
            if nostdlib {
                cmd_append!(cmd, c!("-nostdlib"));
            }
            da_append_many(cmd, linker);
            if !cmd_run_sync_and_reset(cmd) {
                return None;
            }

            Some(())
        }
        Os::Linux => {
            if !(cfg!(target_arch = "x86_64") && cfg!(target_os = "linux")) {
                // TODO: think how to approach cross-compilation
                log(
                    Log_Level::ERROR,
                    c!("Cross-compilation of x86_64 linux is not supported for now"),
                );
                return None;
            }

            let output_obj_path = temp_sprintf(c!("%s.o"), garbage_base);
            cmd_append! {
                cmd,
                c!("as"), output_asm_path, c!("-o"), output_obj_path,
            }
            if !cmd_run_sync_and_reset(cmd) {
                return None;
            }

            cmd_append! {
                cmd,
                c!("cc"), c!("-no-pie"), c!("-o"), program_path, output_obj_path,
            }
            if nostdlib {
                cmd_append!(cmd, c!("-nostdlib"));
            }
            da_append_many(cmd, linker);
            if !cmd_run_sync_and_reset(cmd) {
                return None;
            }

            Some(())
        }
        Os::Windows => {
            let output_obj_path = temp_sprintf(c!("%s.o"), garbage_base);
            cmd_append! {
                cmd,
                c!("as"), output_asm_path, c!("-o"), output_obj_path,
            }
            if !cmd_run_sync_and_reset(cmd) {
                return None;
            }

            cmd_append! {
                cmd,
                c!("x86_64-w64-mingw32-gcc"), c!("-no-pie"), c!("-o"), program_path, output_obj_path,
            }
            if nostdlib {
                cmd_append!(cmd, c!("-nostdlib"));
            }
            da_append_many(cmd, linker);
            if !cmd_run_sync_and_reset(cmd) {
                return None;
            }

            Some(())
        }
    }
}

pub unsafe fn run_program(
    cmd: *mut Cmd,
    program_path: *const c_char,
    run_args: *const [*const c_char],
    stdout_path: Option<*const c_char>,
    os: Os,
) -> Option<()> {
    match os {
        Os::Linux => {
            // if the user does `b program.b -run` the compiler tries to run `program` which is not possible on Linux. It has to be `./program`.
            let run_path: *const c_char;
            if (strchr(program_path, '/' as c_int)).is_null() {
                run_path = temp_sprintf(c!("./%s"), program_path);
            } else {
                run_path = program_path;
            }

            cmd_append! {cmd, run_path}
            da_append_many(cmd, run_args);

            if let Some(stdout_path) = stdout_path {
                let mut fdout = fd_open_for_write(stdout_path);
                let mut redirect: Cmd_Redirect = zeroed();
                redirect.fdout = &mut fdout;
                if !cmd_run_sync_redirect_and_reset(cmd, redirect) {
                    return None;
                }
            } else {
                if !cmd_run_sync_and_reset(cmd) {
                    return None;
                }
            }
            Some(())
        }
        Os::Windows => {
            // TODO: document that you may need wine as a system package to cross-run gas-x86_64-windows
            if !cfg!(target_os = "windows") {
                cmd_append! {
                    cmd,
                    c!("wine"),
                }
            }

            cmd_append! {cmd, program_path}
            da_append_many(cmd, run_args);

            if let Some(stdout_path) = stdout_path {
                let mut fdout = fd_open_for_write(stdout_path);
                let mut redirect: Cmd_Redirect = zeroed();
                redirect.fdout = &mut fdout;
                if !cmd_run_sync_redirect_and_reset(cmd, redirect) {
                    return None;
                }
            } else {
                if !cmd_run_sync_and_reset(cmd) {
                    return None;
                }
            }
            Some(())
        }
        Os::Darwin => {
            if !cfg!(target_os = "macos") {
                log(
                    Log_Level::ERROR,
                    c!("This runner is only for macOS, but the current target is not macOS."),
                );
                return None;
            }

            // if the user does `b program.b -run` the compiler tries to run `program` which is not possible on Darwin. It has to be `./program`.
            let run_path: *const c_char;
            if (strchr(program_path, '/' as c_int)).is_null() {
                run_path = temp_sprintf(c!("./%s"), program_path);
            } else {
                run_path = program_path;
            }

            cmd_append! {cmd, run_path}
            da_append_many(cmd, run_args);

            if let Some(stdout_path) = stdout_path {
                let mut fdout = fd_open_for_write(stdout_path);
                let mut redirect: Cmd_Redirect = zeroed();
                redirect.fdout = &mut fdout;
                if !cmd_run_sync_redirect_and_reset(cmd, redirect) {
                    return None;
                }
            } else {
                if !cmd_run_sync_and_reset(cmd) {
                    return None;
                }
            }
            Some(())
        }
    }
}
