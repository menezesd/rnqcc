// ============================================================
// Target & Compiler Stage
// ============================================================

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

    pub fn show_symbol(&self, name: &str) -> String {
        let name = mangle_assembly_label(name);
        match self.os {
            TargetOs::MacOs => format!("_{}", name),
            TargetOs::Linux => name,
        }
    }

    pub fn show_symbol_with_offset(&self, name: &str, offset: i64) -> String {
        let mut name = self.show_symbol(name);
        name.push_str(&assembly_offset_suffix(offset));
        name
    }

    pub fn show_data_label_expr(&self, name: &str) -> String {
        let Some(offset) = split_data_offset(name) else {
            return self.show_symbol(name);
        };
        self.show_symbol_with_offset(offset.base, offset.offset)
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

fn is_assembly_label_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '.')
}

pub struct DataOffset<'a> {
    pub base: &'a str,
    pub offset: i64,
}

pub fn split_data_offset(name: &str) -> Option<DataOffset<'_>> {
    let pos = name
        .char_indices()
        .rev()
        .find(|(idx, ch)| *idx > 0 && matches!(ch, '+' | '-'))?
        .0;
    let offset = name[pos..].parse().ok()?;
    Some(DataOffset {
        base: &name[..pos],
        offset,
    })
}

pub fn assembly_offset_suffix(offset: i64) -> String {
    if offset >= 0 {
        format!("+{offset}")
    } else {
        offset.to_string()
    }
}

pub fn is_valid_universal_character_value(value: u32) -> bool {
    validate_universal_character_value(value).is_ok()
}

pub fn validate_universal_character_value(value: u32) -> Result<(), &'static str> {
    if matches!(value, 0x24 | 0x40 | 0x60)
        || (0xA0..=0xD7FF).contains(&value)
        || (0xE000..=0x10FFFF).contains(&value)
    {
        Ok(())
    } else if value > 0x10FFFF || (0xD800..=0xDFFF).contains(&value) {
        Err("out-of-range universal character")
    } else {
        Err("basic character universal character")
    }
}

pub fn universal_character_error_message(context: &str, reason: &str) -> String {
    format!("invalid {context}: {reason}")
}

pub fn universal_character_escape_error(reason: &str) -> String {
    universal_character_error_message("universal character escape", reason)
}

pub fn universal_character_name_error(reason: &str) -> String {
    universal_character_error_message("universal character name", reason)
}

fn mangle_assembly_label(name: &str) -> String {
    if name.chars().all(is_assembly_label_char) && !name.starts_with("__rnqcc_u") {
        return name.to_string();
    }

    let mut hash = 0xcbf29ce484222325u64;
    for byte in name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }

    let mut out = String::from("__rnqcc_u");
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if ch == '_' {
            out.push_str("__");
        } else {
            out.push_str("_x");
            out.push_str(&format!("{:x}", ch as u32));
            out.push('_');
        }
    }
    out.push_str("_h");
    out.push_str(&format!("{hash:016x}"));
    out
}
