//! Native shared-library format and architecture preflight for `.node` files.
//!
//! This check is shared by native addon backends so malformed or foreign
//! binaries fail before either Node or the in-process Node-API host runs an
//! initializer.

use std::fs;
use std::io::Read;
#[cfg(target_os = "windows")]
use std::io::{Seek, SeekFrom};
use std::path::Path;

use crate::error::VmErr;

pub(super) fn validate_native_addon_binary(filename: &Path) -> Result<(), VmErr> {
    let display = filename.display().to_string();
    let mut file = fs::File::open(filename)
        .map_err(|error| VmErr::Msg(format!("cannot inspect native addon {display}: {error}")))?;
    let file_length = file
        .metadata()
        .map_err(|error| VmErr::Msg(format!("cannot inspect native addon {display}: {error}")))?
        .len();
    #[cfg(target_os = "windows")]
    let (header, length) = read_pe_header_for_validation(&mut file, file_length)
        .map_err(|error| VmErr::Msg(format!("cannot inspect native addon {display}: {error}")))?;
    #[cfg(not(target_os = "windows"))]
    let (header, length) = {
        let mut header = [0_u8; 4096];
        let length = file.read(&mut header).map_err(|error| {
            VmErr::Msg(format!("cannot inspect native addon {display}: {error}"))
        })?;
        (header, length)
    };
    validate_native_addon_header(
        &header[..length],
        file_length,
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(target_endian = "little"),
    )
    .map_err(|reason| VmErr::Msg(format!("incompatible native addon {display}: {reason}")))
}

#[cfg(target_os = "windows")]
fn read_pe_header_for_validation(
    file: &mut fs::File,
    file_length: u64,
) -> std::io::Result<(Vec<u8>, usize)> {
    let mut dos_header = [0_u8; 64];
    let dos_length = file.read(&mut dos_header)?;
    if dos_length < dos_header.len() || !dos_header.starts_with(b"MZ") {
        return Ok((dos_header[..dos_length].to_vec(), dos_length));
    }
    let Some(pe_offset) = read_u32(&dos_header, 0x3c, true) else {
        return Ok((dos_header.to_vec(), dos_header.len()));
    };
    let Some(pe_end) = u64::from(pe_offset).checked_add(26) else {
        return Ok((dos_header.to_vec(), dos_header.len()));
    };
    if pe_end > file_length {
        return Ok((dos_header.to_vec(), dos_header.len()));
    }

    let validation_pe_offset = dos_header.len();
    let mut header = vec![0_u8; validation_pe_offset + 26];
    header[..dos_header.len()].copy_from_slice(&dos_header);
    header[0x3c..0x40].copy_from_slice(&(validation_pe_offset as u32).to_le_bytes());
    file.seek(SeekFrom::Start(u64::from(pe_offset)))?;
    file.read_exact(&mut header[validation_pe_offset..])?;
    let length = header.len();
    Ok((header, length))
}

pub(super) fn validate_native_addon_header(
    bytes: &[u8],
    file_length: u64,
    host_os: &str,
    host_arch: &str,
    host_is_little_endian: bool,
) -> Result<(), String> {
    match host_os {
        "linux" => validate_elf_addon_header(bytes, file_length, host_arch, host_is_little_endian),
        "macos" => validate_macho_addon_header(bytes, file_length, host_arch),
        "windows" => validate_pe_addon_header(bytes, file_length, host_arch),
        _ => Err(format!(
            "napi-vm native addon loading does not support binaries for {host_os}"
        )),
    }
}

fn validate_pe_addon_header(bytes: &[u8], file_length: u64, host_arch: &str) -> Result<(), String> {
    if !bytes.starts_with(b"MZ") {
        if bytes.starts_with(b"\x7fELF") {
            return Err("found an ELF binary; this Windows host requires PE".into());
        }
        if looks_like_macho(bytes) {
            return Err("found a Mach-O binary; this Windows host requires PE".into());
        }
        return Err("the file is not a PE image".into());
    }
    if bytes.len() < 64 || file_length < 64 {
        return Err("the DOS header is truncated".into());
    }
    let pe_offset = read_u32(bytes, 0x3c, true)
        .ok_or_else(|| "the DOS header is truncated".to_string())? as usize;
    let optional_magic_end = pe_offset
        .checked_add(26)
        .ok_or_else(|| "the PE header offset overflows the file format".to_string())?;
    if optional_magic_end > bytes.len() || optional_magic_end as u64 > file_length {
        return Err("the PE/COFF header is truncated".into());
    }
    if bytes.get(pe_offset..pe_offset + 4) != Some(&b"PE\0\0"[..]) {
        return Err("the PE signature is invalid".into());
    }
    let (expected_machine, expected_magic) = match host_arch {
        "x86" => (0x014c, 0x010b),
        "x86_64" => (0x8664, 0x020b),
        "arm" => (0x01c4, 0x010b),
        "aarch64" => (0xaa64, 0x020b),
        _ => {
            return Err(format!(
                "napi-vm native addon loading does not support PE architecture {host_arch}"
            ));
        }
    };
    let machine = read_u16(bytes, pe_offset + 4, true)
        .ok_or_else(|| "the PE/COFF header is truncated".to_string())?;
    if machine != expected_machine {
        return Err(format!(
            "PE architecture {} does not match host architecture {host_arch}",
            pe_architecture_name(machine)
        ));
    }
    let characteristics = read_u16(bytes, pe_offset + 22, true)
        .ok_or_else(|| "the PE/COFF header is truncated".to_string())?;
    if characteristics & 0x2000 == 0 {
        return Err("the PE image is not a DLL".into());
    }
    let optional_magic = read_u16(bytes, pe_offset + 24, true)
        .ok_or_else(|| "the PE optional header is truncated".to_string())?;
    if optional_magic != expected_magic {
        return Err(format!(
            "PE optional-header format {optional_magic:#06x} does not match host architecture {host_arch}"
        ));
    }
    Ok(())
}

fn pe_architecture_name(machine: u16) -> String {
    match machine {
        0x014c => "x86".into(),
        0x8664 => "x86_64".into(),
        0x01c4 => "armv7".into(),
        0xaa64 => "aarch64".into(),
        _ => format!("PE machine {machine:#06x}"),
    }
}

fn validate_elf_addon_header(
    bytes: &[u8],
    file_length: u64,
    host_arch: &str,
    host_is_little_endian: bool,
) -> Result<(), String> {
    if !bytes.starts_with(b"\x7fELF") {
        if looks_like_macho(bytes) {
            return Err("found a Mach-O binary; this Linux host requires ELF".into());
        }
        return Err("the file is not an ELF shared library".into());
    }
    if bytes.len() < 20 {
        return Err("the ELF header is truncated".into());
    }
    let (expected_machine, expected_64_bit) = elf_architecture(host_arch).ok_or_else(|| {
        format!("napi-vm native addon loading does not support ELF architecture {host_arch}")
    })?;
    let class = match bytes[4] {
        1 => false,
        2 => true,
        _ => return Err(format!("the ELF class value {} is invalid", bytes[4])),
    };
    let required_header_length = if class { 64 } else { 52 };
    if bytes.len() < required_header_length || file_length < required_header_length as u64 {
        return Err("the ELF header is truncated".into());
    }
    if class != expected_64_bit {
        return Err(format!(
            "ELF class does not match host architecture {host_arch}"
        ));
    }
    let little_endian = match bytes[5] {
        1 => true,
        2 => false,
        _ => return Err(format!("the ELF byte-order value {} is invalid", bytes[5])),
    };
    if little_endian != host_is_little_endian {
        return Err("ELF byte order does not match the host".into());
    }
    let header_size_offset = if class { 52 } else { 40 };
    let header_size = read_u16(bytes, header_size_offset, little_endian)
        .ok_or_else(|| "the ELF header is truncated".to_string())?;
    if header_size as usize != required_header_length {
        return Err(format!(
            "ELF header size {header_size} does not match class size {required_header_length}"
        ));
    }
    let file_type = read_u16(bytes, 16, little_endian)
        .ok_or_else(|| "the ELF header is truncated".to_string())?;
    if file_type != 3 {
        return Err(format!(
            "ELF file type {file_type} is not a shared object (ET_DYN)"
        ));
    }
    let machine = read_u16(bytes, 18, little_endian)
        .ok_or_else(|| "the ELF header is truncated".to_string())?;
    if machine != expected_machine {
        return Err(format!(
            "ELF architecture {} does not match host architecture {host_arch}",
            elf_architecture_name(machine)
        ));
    }
    Ok(())
}

fn validate_macho_addon_header(
    bytes: &[u8],
    file_length: u64,
    host_arch: &str,
) -> Result<(), String> {
    if !looks_like_macho(bytes) {
        if bytes.starts_with(b"\x7fELF") {
            return Err("found an ELF binary; this macOS host requires Mach-O".into());
        }
        return Err("the file is not a Mach-O shared library".into());
    }
    let expected_cpu = macho_cpu_type(host_arch).ok_or_else(|| {
        format!("napi-vm native addon loading does not support Mach-O architecture {host_arch}")
    })?;
    let magic =
        read_u32(bytes, 0, false).ok_or_else(|| "the Mach-O header is truncated".to_string())?;
    match magic {
        0xfeed_face | 0xfeed_facf | 0xcefa_edfe | 0xcffa_edfe => {
            validate_thin_macho(bytes, expected_cpu, host_arch)
        }
        0xcafe_babe | 0xcafe_babf | 0xbeba_feca | 0xbfba_feca => {
            validate_fat_macho(bytes, file_length, expected_cpu, host_arch)
        }
        _ => Err("the Mach-O magic value is invalid".into()),
    }
}

fn validate_thin_macho(bytes: &[u8], expected_cpu: u32, host_arch: &str) -> Result<(), String> {
    let magic =
        read_u32(bytes, 0, false).ok_or_else(|| "the Mach-O header is truncated".to_string())?;
    let little_endian = matches!(magic, 0xcefa_edfe | 0xcffa_edfe);
    let is_64_bit = matches!(magic, 0xfeed_facf | 0xcffa_edfe);
    let host_is_64_bit = matches!(host_arch, "x86_64" | "aarch64");
    if is_64_bit != host_is_64_bit {
        return Err(format!(
            "Mach-O class does not match host architecture {host_arch}"
        ));
    }
    let required_length = if is_64_bit { 32 } else { 28 };
    if bytes.len() < required_length {
        return Err("the Mach-O header is truncated".into());
    }
    let cpu_type = read_u32(bytes, 4, little_endian)
        .ok_or_else(|| "the Mach-O header is truncated".to_string())?;
    if cpu_type != expected_cpu {
        return Err(format!(
            "Mach-O architecture {} does not match host architecture {host_arch}",
            macho_architecture_name(cpu_type)
        ));
    }
    validate_macho_file_type(bytes, little_endian)
}

fn validate_fat_macho(
    bytes: &[u8],
    file_length: u64,
    expected_cpu: u32,
    host_arch: &str,
) -> Result<(), String> {
    let magic = read_u32(bytes, 0, false)
        .ok_or_else(|| "the universal Mach-O header is truncated".to_string())?;
    let little_endian = matches!(magic, 0xbeba_feca | 0xbfba_feca);
    let is_64_bit = matches!(magic, 0xcafe_babf | 0xbfba_feca);
    let architecture_count = read_u32(bytes, 4, little_endian)
        .ok_or_else(|| "the universal Mach-O header is truncated".to_string())?;
    if architecture_count == 0 || architecture_count > 64 {
        return Err(format!(
            "universal Mach-O architecture count {architecture_count} is invalid"
        ));
    }
    let entry_length = if is_64_bit { 32 } else { 20 };
    let table_length = 8 + architecture_count as usize * entry_length;
    if bytes.len() < table_length {
        return Err("the universal Mach-O architecture table is truncated".into());
    }
    let mut found_host_arch = false;
    for index in 0..architecture_count as usize {
        let start = 8 + index * entry_length;
        let cpu_type = read_u32(bytes, start, little_endian)
            .ok_or_else(|| "the universal Mach-O architecture table is truncated".to_string())?;
        let (offset, size) = if is_64_bit {
            (
                read_u64(bytes, start + 8, little_endian),
                read_u64(bytes, start + 16, little_endian),
            )
        } else {
            (
                read_u32(bytes, start + 8, little_endian).map(u64::from),
                read_u32(bytes, start + 12, little_endian).map(u64::from),
            )
        };
        let (Some(offset), Some(size)) = (offset, size) else {
            return Err("the universal Mach-O architecture table is truncated".into());
        };
        if offset.checked_add(size).is_none_or(|end| end > file_length) {
            return Err("a universal Mach-O architecture slice extends past end of file".into());
        }
        if cpu_type == expected_cpu {
            found_host_arch = true;
        }
    }
    if !found_host_arch {
        let available = (0..architecture_count as usize)
            .filter_map(|index| read_u32(bytes, 8 + index * entry_length, little_endian))
            .map(macho_architecture_name)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "universal Mach-O contains [{available}] but host architecture is {host_arch}"
        ));
    }
    Ok(())
}

fn validate_macho_file_type(bytes: &[u8], little_endian: bool) -> Result<(), String> {
    let file_type = read_u32(bytes, 12, little_endian)
        .ok_or_else(|| "the Mach-O header is truncated".to_string())?;
    if matches!(file_type, 6 | 8) {
        Ok(())
    } else {
        Err(format!(
            "Mach-O file type {file_type} is neither a dylib nor a bundle"
        ))
    }
}

fn looks_like_macho(bytes: &[u8]) -> bool {
    read_u32(bytes, 0, false).is_some_and(|magic| {
        matches!(
            magic,
            0xfeed_face
                | 0xfeed_facf
                | 0xcefa_edfe
                | 0xcffa_edfe
                | 0xcafe_babe
                | 0xcafe_babf
                | 0xbeba_feca
                | 0xbfba_feca
        )
    })
}

fn read_u16(bytes: &[u8], offset: usize, little_endian: bool) -> Option<u16> {
    let bytes: [u8; 2] = bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?;
    Some(if little_endian {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    })
}

fn read_u32(bytes: &[u8], offset: usize, little_endian: bool) -> Option<u32> {
    let bytes: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    Some(if little_endian {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    })
}

fn read_u64(bytes: &[u8], offset: usize, little_endian: bool) -> Option<u64> {
    let bytes: [u8; 8] = bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?;
    Some(if little_endian {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    })
}

fn elf_architecture(arch: &str) -> Option<(u16, bool)> {
    match arch {
        "x86" => Some((3, false)),
        "x86_64" => Some((62, true)),
        "arm" => Some((40, false)),
        "aarch64" => Some((183, true)),
        "powerpc" => Some((20, false)),
        "powerpc64" | "powerpc64le" => Some((21, true)),
        "s390x" => Some((22, true)),
        "sparc64" => Some((43, true)),
        "mips" => Some((8, false)),
        "mips64" => Some((8, true)),
        "riscv32" => Some((243, false)),
        "riscv64" => Some((243, true)),
        "loongarch64" => Some((258, true)),
        _ => None,
    }
}

fn elf_architecture_name(machine: u16) -> String {
    match machine {
        3 => "x86".into(),
        40 => "arm".into(),
        62 => "x86_64".into(),
        183 => "aarch64".into(),
        20 => "powerpc".into(),
        21 => "powerpc64".into(),
        22 => "s390x".into(),
        43 => "sparc64".into(),
        8 => "mips".into(),
        243 => "riscv".into(),
        258 => "loongarch64".into(),
        _ => format!("ELF machine {machine}"),
    }
}

fn macho_cpu_type(arch: &str) -> Option<u32> {
    match arch {
        "x86" => Some(7),
        "x86_64" => Some(0x0100_0007),
        "arm" => Some(12),
        "aarch64" => Some(0x0100_000c),
        _ => None,
    }
}

fn macho_architecture_name(cpu_type: u32) -> String {
    match cpu_type {
        7 => "x86".into(),
        0x0100_0007 => "x86_64".into(),
        12 => "arm".into(),
        0x0100_000c => "aarch64".into(),
        _ => format!("Mach-O CPU type {cpu_type}"),
    }
}

#[cfg(test)]
mod tests {
    use super::validate_native_addon_header;

    #[test]
    fn native_addon_binary_preflight_checks_format_and_architecture() {
        fn elf_header(class: u8, machine: u16) -> Vec<u8> {
            let is_64_bit = class == 2;
            let mut header = vec![0_u8; if is_64_bit { 64 } else { 52 }];
            header[..4].copy_from_slice(b"\x7fELF");
            header[4] = class;
            header[5] = 1;
            header[16..18].copy_from_slice(&3_u16.to_le_bytes());
            header[18..20].copy_from_slice(&machine.to_le_bytes());
            let header_size_offset = if is_64_bit { 52 } else { 40 };
            let header_size = if is_64_bit { 64_u16 } else { 52_u16 };
            header[header_size_offset..header_size_offset + 2]
                .copy_from_slice(&header_size.to_le_bytes());
            header
        }

        let valid_elf = elf_header(2, 62);
        assert!(validate_native_addon_header(&valid_elf, 64, "linux", "x86_64", true).is_ok());

        let wrong_arch = elf_header(2, 183);
        assert!(
            validate_native_addon_header(&wrong_arch, 64, "linux", "x86_64", true)
                .unwrap_err()
                .contains("ELF architecture aarch64 does not match host architecture x86_64")
        );

        let wrong_class = elf_header(1, 3);
        assert!(
            validate_native_addon_header(&wrong_class, 52, "linux", "x86_64", true)
                .unwrap_err()
                .contains("ELF class does not match host architecture x86_64")
        );

        let mut thin_macho = vec![0_u8; 32];
        thin_macho[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        thin_macho[4..8].copy_from_slice(&0x0100_000c_u32.to_le_bytes());
        thin_macho[12..16].copy_from_slice(&8_u32.to_le_bytes());
        assert!(validate_native_addon_header(&thin_macho, 32, "macos", "aarch64", true).is_ok());
        assert!(
            validate_native_addon_header(&thin_macho, 32, "macos", "x86_64", true)
                .unwrap_err()
                .contains("Mach-O architecture aarch64 does not match host architecture x86_64")
        );
        assert!(
            validate_native_addon_header(&thin_macho, 32, "linux", "x86_64", true)
                .unwrap_err()
                .contains("found a Mach-O binary")
        );
        let mut wrong_class_macho = thin_macho.clone();
        wrong_class_macho[..4].copy_from_slice(&[0xce, 0xfa, 0xed, 0xfe]);
        assert!(
            validate_native_addon_header(&wrong_class_macho, 32, "macos", "aarch64", true)
                .unwrap_err()
                .contains("Mach-O class does not match host architecture aarch64")
        );

        let mut universal_macho = vec![0_u8; 28];
        universal_macho[..4].copy_from_slice(&[0xca, 0xfe, 0xba, 0xbe]);
        universal_macho[4..8].copy_from_slice(&1_u32.to_be_bytes());
        universal_macho[8..12].copy_from_slice(&0x0100_0007_u32.to_be_bytes());
        universal_macho[16..20].copy_from_slice(&28_u32.to_be_bytes());
        universal_macho[20..24].copy_from_slice(&100_u32.to_be_bytes());
        assert!(
            validate_native_addon_header(&universal_macho, 128, "macos", "x86_64", true).is_ok()
        );

        assert!(
            validate_native_addon_header(b"bad", 3, "linux", "x86_64", true)
                .unwrap_err()
                .contains("not an ELF shared library")
        );
        assert!(
            validate_native_addon_header(&valid_elf[..12], 12, "linux", "x86_64", true)
                .unwrap_err()
                .contains("ELF header is truncated")
        );
    }
}
