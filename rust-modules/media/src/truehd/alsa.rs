//! The eARC output's own word on what it carries: the ALSA controls `Sndout EARC OnOff` and
//! `Sndout EARC Outputtype` on card 0, read the way dts_restore_plus's `lgroute.c` reads them.
//! Outputtype's fifth word is 1 while LG mixes, and the carrier must never reach a mix.
use std::os::raw::c_long;

/// The two controls' last values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Earc {
    pub on_off: c_long,
    pub output_type: [c_long; 5],
}

impl Earc {
    /// eARC is on and its output type is one upstream recognises, unmixed.
    pub fn passes(&self) -> bool {
        let w = self.output_type;
        self.on_off == 1
            && (0..=2).contains(&w[0])
            && (1..=9).contains(&w[1])
            && (0..=8).contains(&w[2])
            && (0..=192_000).contains(&w[3])
            && w[4] == 0
    }
}

#[cfg(target_os = "linux")]
pub use imp::Controls;

#[cfg(target_os = "linux")]
mod imp {
    use super::Earc;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::raw::{c_long, c_uint};

    const IFACE_MIXER: i32 = 2;
    const TYPE_INTEGER: i32 = 2;
    const ACCESS_READ: c_uint = 1;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ElemId {
        numid: c_uint,
        _iface: i32,
        _device: c_uint,
        _subdevice: c_uint,
        name: [u8; 44],
        _index: c_uint,
    }

    /// `struct snd_ctl_elem_info`: 272 bytes on ARM32 and x86-64.
    #[repr(C)]
    struct ElemInfo {
        id: ElemId,
        kind: i32,
        access: c_uint,
        count: c_uint,
        _owner: i32,
        _value: [u64; 16],
        _reserved: [u8; 64],
    }

    /// `struct snd_ctl_elem_value`: the id, the `indirect` bit, then the value union, which its
    /// `long long` member puts 8-aligned at 72 (`long integer[128]`), then a timespec and reserved
    /// bytes to 712 (ARM32) or 1224 (x86-64).
    #[repr(C)]
    struct ElemValue {
        id: ElemId,
        _indirect: c_uint,
        integer: Integers,
        _tail: [u8; 128],
    }

    #[repr(C, align(8))]
    struct Integers([c_long; 128]);

    const _: () = assert!(std::mem::size_of::<ElemInfo>() == 272);
    const _: () = assert!(std::mem::offset_of!(ElemValue, integer) == 72);

    const fn iowr(nr: u32, size: usize) -> u32 {
        (3 << 30) | ((size as u32) << 16) | ((b'U' as u32) << 8) | nr
    }
    const ELEM_INFO: u32 = iowr(0x11, std::mem::size_of::<ElemInfo>());
    const ELEM_READ: u32 = iowr(0x12, std::mem::size_of::<ElemValue>());

    /// Card 0's control device with the two controls found.
    pub struct Controls {
        fd: OwnedFd,
        on_off: c_uint,
        output_type: c_uint,
    }

    impl Controls {
        /// `None` when the device or either control is not there in the shape upstream expects.
        pub fn open() -> Option<Controls> {
            let fd = unsafe { libc::open(c"/dev/snd/controlC0".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
            if fd < 0 {
                return None;
            }
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            let on_off = find(&fd, b"Sndout EARC OnOff", 1)?;
            let output_type = find(&fd, b"Sndout EARC Outputtype", 5)?;
            Some(Controls { fd, on_off, output_type })
        }

        pub fn read(&self) -> Option<Earc> {
            let on = read(&self.fd, self.on_off)?;
            let out = read(&self.fd, self.output_type)?;
            Some(Earc { on_off: on[0], output_type: out })
        }
    }

    fn id(name: &[u8]) -> ElemId {
        let mut id = ElemId { numid: 0, _iface: IFACE_MIXER, _device: 0, _subdevice: 0, name: [0; 44], _index: 0 };
        id.name[..name.len()].copy_from_slice(name);
        id
    }

    /// The control's number, if it is a readable mixer integer of `count` values.
    fn find(fd: &OwnedFd, name: &[u8], count: c_uint) -> Option<c_uint> {
        let mut info: ElemInfo = unsafe { std::mem::zeroed() };
        info.id = id(name);
        if unsafe { libc::ioctl(fd.as_raw_fd(), ELEM_INFO as _, &mut info) } != 0 {
            return None;
        }
        (info.kind == TYPE_INTEGER && info.count == count && info.access & ACCESS_READ != 0).then_some(info.id.numid)
    }

    fn read(fd: &OwnedFd, numid: c_uint) -> Option<[c_long; 5]> {
        let mut value: ElemValue = unsafe { std::mem::zeroed() };
        value.id.numid = numid;
        if unsafe { libc::ioctl(fd.as_raw_fd(), ELEM_READ as _, &mut value) } != 0 {
            return None;
        }
        Some(std::array::from_fn(|i| value.integer.0[i]))
    }

    #[cfg(test)]
    #[test]
    fn the_ioctls_match_the_kernel_s_numbers() {
        assert_eq!(ELEM_INFO, 0xC110_5511);
        let read = if cfg!(target_pointer_width = "32") { 0xC2C8_5512 } else { 0xC4C8_5512 };
        assert_eq!(ELEM_READ, read);
    }
}

/// The controls exist on the television alone.
#[cfg(not(target_os = "linux"))]
pub struct Controls;

#[cfg(not(target_os = "linux"))]
impl Controls {
    pub fn open() -> Option<Controls> {
        None
    }

    pub fn read(&self) -> Option<Earc> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_unmixed_valid_output_on_earc_passes() {
        let ok = Earc { on_off: 1, output_type: [2, 7, 0, 0, 0] };
        assert!(ok.passes());
        // what the set read while LG mixed Dolby Digital Plus
        assert!(!Earc { output_type: [2, 7, 0, 0, 1], ..ok }.passes());
        assert!(!Earc { on_off: 0, ..ok }.passes());
        assert!(!Earc { output_type: [3, 7, 0, 0, 0], ..ok }.passes(), "a word outside upstream's range");
        assert!(!Earc { output_type: [2, 0, 0, 0, 0], ..ok }.passes());
        assert!(!Earc { output_type: [2, 7, 0, 192_001, 0], ..ok }.passes());
    }
}
