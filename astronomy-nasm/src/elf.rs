//! ELF64 relocatable object writer (x86-64, System V / Linux).
//!
//! Produces a `.o` file that `ld`/`gcc`/`clang` link like any compiler
//! output:
//!
//! | Section           | Contents                                          |
//! |-------------------|---------------------------------------------------|
//! | `.text`           | machine code of every defined function            |
//! | `.rodata`         | string literals and float-conversion bounds       |
//! | `.note.GNU-stack` | empty marker: the code needs no executable stack  |
//! | `.rela.text`      | `PLT32` calls to declarations, `PC32` data refs   |
//! | `.symtab`/`.strtab` | function symbols (`STT_FUNC` with sizes)        |
//! | `.shstrtab`       | section names                                     |
//!
//! Internal-linkage functions become local symbols, exported/external
//! definitions global ones, and only declarations that are actually called
//! appear as undefined symbols. Calls to declarations use `R_X86_64_PLT32`
//! and data is reached RIP-relatively, so the object links into both PIE
//! and non-PIE executables and into shared libraries. Output is
//! deterministic: no timestamps, no paths, no host-dependent data.

use crate::asm::{rodata_layout, Program, SymbolKind};
use crate::encode::{RelocTarget, Text};
use crate::error::BackendError;

const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_RELA: u32 = 4;

const SHF_ALLOC: u64 = 0x2;
const SHF_EXECINSTR: u64 = 0x4;
const SHF_INFO_LINK: u64 = 0x40;

const STB_LOCAL: u8 = 0;
const STB_GLOBAL: u8 = 1;
const STT_NOTYPE: u8 = 0;
const STT_FUNC: u8 = 2;
const STT_SECTION: u8 = 3;

const R_X86_64_PC32: u32 = 2;
const R_X86_64_PLT32: u32 = 4;

const EM_X86_64: u16 = 62;
const ET_REL: u16 = 1;

/// A string table under construction (index 0 is the empty string).
struct StrTab(Vec<u8>);

impl StrTab {
    fn new() -> Self {
        StrTab(vec![0])
    }

    fn add(&mut self, s: &str) -> u32 {
        let at = self.0.len() as u32;
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
        at
    }
}

struct Symbol {
    name: u32,
    info: u8,
    shndx: u16,
    value: u64,
    size: u64,
}

struct Section {
    name: u32,
    kind: u32,
    flags: u64,
    data: Vec<u8>,
    link: u32,
    info: u32,
    align: u64,
    entsize: u64,
}

/// Serializes assembled code and data into an ELF64 relocatable object.
pub fn write(program: &Program, text: &Text) -> Result<Vec<u8>, BackendError> {
    for symbol in &program.symbols {
        if symbol.name.is_empty() || symbol.name.contains('\0') {
            return Err(BackendError::ObjectLimit {
                reason: format!(
                    "symbol name {:?} cannot be represented in an ELF string table",
                    symbol.name
                ),
            });
        }
    }

    // -- section numbering ----------------------------------------------------
    let has_rodata = !program.rodata.is_empty();
    let has_relocs = !text.relocs.is_empty();
    let mut next = 1u16;
    let mut number = |present: bool| {
        present.then(|| {
            let n = next;
            next += 1;
            n
        })
    };
    let text_ndx = number(true).expect("always present");
    let rodata_ndx = number(has_rodata);
    number(true); // .note.GNU-stack
    number(has_relocs); // .rela.text
    let symtab_ndx = number(true).expect("always present");
    let strtab_ndx = number(true).expect("always present");
    let shstrtab_ndx = number(true).expect("always present");
    let section_count = next;

    // -- symbols --------------------------------------------------------------
    let mut strtab = StrTab::new();
    let mut symbols = vec![Symbol {
        name: 0,
        info: 0,
        shndx: 0,
        value: 0,
        size: 0,
    }];
    let section_symbol = |shndx| Symbol {
        name: 0,
        info: STB_LOCAL << 4 | STT_SECTION,
        shndx,
        value: 0,
        size: 0,
    };
    symbols.push(section_symbol(text_ndx));
    let rodata_sym = rodata_ndx.map(|ndx| {
        symbols.push(section_symbol(ndx));
        symbols.len() as u32 - 1
    });

    let mut placed = vec![None; program.symbols.len()];
    for &(id, start, size) in &text.functions {
        placed[id.index()] = Some((start, size));
    }
    let called: Vec<bool> = {
        let mut called = vec![false; program.symbols.len()];
        for reloc in &text.relocs {
            if let RelocTarget::Function(id) = reloc.target {
                called[id.index()] = true;
            }
        }
        called
    };

    // Locals must precede globals; within each group, module order.
    let mut sym_index = vec![0u32; program.symbols.len()];
    for (i, symbol) in program.symbols.iter().enumerate() {
        if symbol.kind == SymbolKind::Local {
            let (value, size) = placed[i].expect("defined function has code");
            sym_index[i] = symbols.len() as u32;
            symbols.push(Symbol {
                name: strtab.add(&symbol.name),
                info: STB_LOCAL << 4 | STT_FUNC,
                shndx: text_ndx,
                value,
                size,
            });
        }
    }
    let first_global = symbols.len() as u32;
    for (i, symbol) in program.symbols.iter().enumerate() {
        let entry = match symbol.kind {
            SymbolKind::Local => continue,
            SymbolKind::Global => {
                let (value, size) = placed[i].expect("defined function has code");
                Symbol {
                    name: strtab.add(&symbol.name),
                    info: STB_GLOBAL << 4 | STT_FUNC,
                    shndx: text_ndx,
                    value,
                    size,
                }
            }
            SymbolKind::Extern if called[i] => Symbol {
                name: strtab.add(&symbol.name),
                info: STB_GLOBAL << 4 | STT_NOTYPE,
                shndx: 0,
                value: 0,
                size: 0,
            },
            SymbolKind::Extern => continue,
        };
        sym_index[i] = symbols.len() as u32;
        symbols.push(entry);
    }

    let mut symtab = Vec::with_capacity(symbols.len() * 24);
    for s in &symbols {
        symtab.extend_from_slice(&s.name.to_le_bytes());
        symtab.push(s.info);
        symtab.push(0); // st_other: STV_DEFAULT
        symtab.extend_from_slice(&s.shndx.to_le_bytes());
        symtab.extend_from_slice(&s.value.to_le_bytes());
        symtab.extend_from_slice(&s.size.to_le_bytes());
    }

    // -- relocations ----------------------------------------------------------
    let mut rela = Vec::with_capacity(text.relocs.len() * 24);
    for reloc in &text.relocs {
        let (sym, kind) = match reloc.target {
            RelocTarget::Function(id) => (sym_index[id.index()], R_X86_64_PLT32),
            RelocTarget::Rodata => (
                rodata_sym.ok_or_else(|| BackendError::InvalidModule {
                    reason: "data reference without read-only data".into(),
                })?,
                R_X86_64_PC32,
            ),
        };
        rela.extend_from_slice(&reloc.offset.to_le_bytes());
        rela.extend_from_slice(&((sym as u64) << 32 | kind as u64).to_le_bytes());
        rela.extend_from_slice(&reloc.addend.to_le_bytes());
    }

    // -- read-only data -------------------------------------------------------
    let (_, rodata_size) = rodata_layout(&program.rodata);
    let mut rodata = Vec::with_capacity(rodata_size as usize);
    for bytes in &program.rodata {
        if bytes.is_empty() {
            rodata.push(0);
        } else {
            rodata.extend_from_slice(bytes);
        }
    }

    // -- sections ---------------------------------------------------------------
    let mut shstrtab = StrTab::new();
    let mut sections = vec![Section {
        name: shstrtab.add(".text"),
        kind: SHT_PROGBITS,
        flags: SHF_ALLOC | SHF_EXECINSTR,
        data: text.bytes.clone(),
        link: 0,
        info: 0,
        align: 16,
        entsize: 0,
    }];
    if has_rodata {
        sections.push(Section {
            name: shstrtab.add(".rodata"),
            kind: SHT_PROGBITS,
            flags: SHF_ALLOC,
            data: rodata,
            link: 0,
            info: 0,
            align: 8,
            entsize: 0,
        });
    }
    sections.push(Section {
        name: shstrtab.add(".note.GNU-stack"),
        kind: SHT_PROGBITS,
        flags: 0,
        data: Vec::new(),
        link: 0,
        info: 0,
        align: 1,
        entsize: 0,
    });
    if has_relocs {
        sections.push(Section {
            name: shstrtab.add(".rela.text"),
            kind: SHT_RELA,
            flags: SHF_INFO_LINK,
            data: rela,
            link: symtab_ndx as u32,
            info: text_ndx as u32,
            align: 8,
            entsize: 24,
        });
    }
    sections.push(Section {
        name: shstrtab.add(".symtab"),
        kind: SHT_SYMTAB,
        flags: 0,
        data: symtab,
        link: strtab_ndx as u32,
        info: first_global,
        align: 8,
        entsize: 24,
    });
    sections.push(Section {
        name: shstrtab.add(".strtab"),
        kind: SHT_STRTAB,
        flags: 0,
        data: strtab.0,
        link: 0,
        info: 0,
        align: 1,
        entsize: 0,
    });
    let shstrtab_name = shstrtab.add(".shstrtab");
    sections.push(Section {
        name: shstrtab_name,
        kind: SHT_STRTAB,
        flags: 0,
        data: shstrtab.0,
        link: 0,
        info: 0,
        align: 1,
        entsize: 0,
    });
    debug_assert_eq!(sections.len() + 1, section_count as usize);

    // -- file layout: header, section contents, section header table ----------
    const EHDR_SIZE: usize = 64;
    const SHDR_SIZE: usize = 64;
    let mut out = vec![0u8; EHDR_SIZE];
    let mut offsets = Vec::with_capacity(sections.len());
    for section in &sections {
        pad_to(&mut out, section.align as usize);
        offsets.push(out.len() as u64);
        out.extend_from_slice(&section.data);
    }
    pad_to(&mut out, 8);
    let shoff = out.len() as u64;

    out.extend_from_slice(&[0u8; SHDR_SIZE]); // SHN_UNDEF
    for (section, &offset) in sections.iter().zip(&offsets) {
        out.extend_from_slice(&section.name.to_le_bytes());
        out.extend_from_slice(&section.kind.to_le_bytes());
        out.extend_from_slice(&section.flags.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes()); // sh_addr
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&(section.data.len() as u64).to_le_bytes());
        out.extend_from_slice(&section.link.to_le_bytes());
        out.extend_from_slice(&section.info.to_le_bytes());
        out.extend_from_slice(&section.align.to_le_bytes());
        out.extend_from_slice(&section.entsize.to_le_bytes());
    }

    let mut header = Vec::with_capacity(EHDR_SIZE);
    header.extend_from_slice(&[0x7F, b'E', b'L', b'F']);
    header.push(2); // ELFCLASS64
    header.push(1); // ELFDATA2LSB
    header.push(1); // EV_CURRENT
    header.push(0); // ELFOSABI_SYSV
    header.extend_from_slice(&[0; 8]); // ABI version + padding
    header.extend_from_slice(&ET_REL.to_le_bytes());
    header.extend_from_slice(&EM_X86_64.to_le_bytes());
    header.extend_from_slice(&1u32.to_le_bytes()); // e_version
    header.extend_from_slice(&0u64.to_le_bytes()); // e_entry
    header.extend_from_slice(&0u64.to_le_bytes()); // e_phoff
    header.extend_from_slice(&shoff.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    header.extend_from_slice(&(EHDR_SIZE as u16).to_le_bytes());
    header.extend_from_slice(&0u16.to_le_bytes()); // e_phentsize
    header.extend_from_slice(&0u16.to_le_bytes()); // e_phnum
    header.extend_from_slice(&(SHDR_SIZE as u16).to_le_bytes());
    header.extend_from_slice(&section_count.to_le_bytes());
    header.extend_from_slice(&shstrtab_ndx.to_le_bytes());
    debug_assert_eq!(header.len(), EHDR_SIZE);
    out[..EHDR_SIZE].copy_from_slice(&header);
    Ok(out)
}

fn pad_to(out: &mut Vec<u8>, align: usize) {
    while !out.len().is_multiple_of(align) {
        out.push(0);
    }
}
