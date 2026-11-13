//! The OEM image code under Unicorn (ports `tools/oem_dice.py` and
//! `tools/oem_sba.py`): Applied Science Fiction's Digital ICE
//! (`oem/DMLDICELib.dll`) and Kodak's Ansel scene and roll balance
//! (`oem/PakonIMAu.dll`), 32-bit x86 run in an emulator with stubbed imports.
//! Running them is for interoperability; the DLLs stay as shipped.

pub mod dice;
mod emu;
pub mod pe;
pub mod sba;
