use crate::Result;
use serde::Serialize;
use std::{
    fmt, io,
    str::FromStr,
    sync::atomic::{AtomicBool, Ordering},
};
use windows_sys::Win32::{
    Foundation::STATUS_TIMER_RESOLUTION_NOT_SET,
    Media::{timeBeginPeriod, timeEndPeriod},
    System::{
        SystemInformation::OSVERSIONINFOW,
        Threading::{
            GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
            ProcessPowerThrottling, SetProcessInformation,
        },
    },
};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryTimerResolution(coarsest: *mut u32, finest: *mut u32, current: *mut u32) -> i32;
    fn NtSetTimerResolution(desired: u32, set: u8, current: *mut u32) -> i32;
    fn RtlGetVersion(version: *mut OSVERSIONINFOW) -> i32;
}

fn nt_status(operation: &str, status: i32) -> Result<()> {
    if status < 0 {
        return Err(format!("{operation} failed (NTSTATUS 0x{:08X})", status as u32).into());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Interval(pub u32);

impl fmt::Display for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:04}", self.0 / 10_000, self.0 % 10_000)
    }
}

impl FromStr for Interval {
    type Err = &'static str;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        const ERROR: &str =
            "Enter a positive interval in ms with up to four decimals (e.g. 0.5100)";
        let (whole, fraction) = s.trim().split_once('.').unwrap_or((s.trim(), ""));
        if whole.is_empty()
            || fraction.len() > 4
            || !whole
                .bytes()
                .chain(fraction.bytes())
                .all(|b| b.is_ascii_digit())
        {
            return Err(ERROR);
        }
        let whole = whole.parse::<u32>().map_err(|_| ERROR)?;
        let fraction_ticks = fraction
            .bytes()
            .fold(0, |acc, b| acc * 10 + u32::from(b - b'0'))
            * 10u32.pow(4 - fraction.len() as u32);
        whole
            .checked_mul(10_000)
            .and_then(|ticks| ticks.checked_add(fraction_ticks))
            .filter(|&ticks| ticks > 0)
            .map(Self)
            .ok_or(ERROR)
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Snapshot {
    pub finest_100ns: Interval,
    pub coarsest_100ns: Interval,
    pub current_100ns: Interval,
}

pub fn query() -> Result<Snapshot> {
    let (mut coarsest, mut finest, mut current) = (0, 0, 0);
    // The three output pointers are valid for one ULONG each.
    let status = unsafe { NtQueryTimerResolution(&mut coarsest, &mut finest, &mut current) };
    nt_status("NtQueryTimerResolution", status)?;
    if finest == 0 || coarsest < finest || current == 0 {
        return Err("Windows returned an invalid timer interval".into());
    }
    Ok(Snapshot {
        finest_100ns: Interval(finest),
        coarsest_100ns: Interval(coarsest),
        current_100ns: Interval(current),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    Baseline,
    Fastest,
    Native(Interval),
    WinMm(u32),
}

impl fmt::Display for Case {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Baseline => f.write_str("baseline"),
            Self::Fastest => f.write_str("fastest"),
            Self::Native(value) => write!(f, "{value}"),
            Self::WinMm(ms) => write!(f, "winmm:{ms}"),
        }
    }
}

impl FromStr for Case {
    type Err = &'static str;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let s = s.trim();
        match s {
            "baseline" => Ok(Self::Baseline),
            "fastest" => Ok(Self::Fastest),
            _ => match s.strip_prefix("winmm:") {
                Some(ms) => ms
                    .parse::<u32>()
                    .ok()
                    .filter(|&n| n > 0)
                    .map(Self::WinMm)
                    .ok_or("Use winmm:<positive whole milliseconds>"),
                None => s.parse().map(Self::Native),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "api", rename_all = "snake_case")]
pub enum Request {
    None,
    Native { interval_100ns: Interval },
    WinMm { period_ms: u32 },
}

impl Request {
    pub fn resolve(case: Case, range: Snapshot) -> Result<Self> {
        let request = match case {
            Case::Baseline => Self::None,
            Case::Fastest => Self::Native {
                interval_100ns: range.finest_100ns,
            },
            Case::Native(interval_100ns) => {
                if !(range.finest_100ns.0..=range.coarsest_100ns.0).contains(&interval_100ns.0) {
                    return Err(format!(
                        "Choose a value from {} to {} ms",
                        range.finest_100ns, range.coarsest_100ns,
                    )
                    .into());
                }
                Self::Native { interval_100ns }
            }
            Case::WinMm(period_ms) => {
                let mut caps = windows_sys::Win32::Media::TIMECAPS::default();
                // timeGetDevCaps writes a TIMECAPS with the supplied byte size.
                let code = unsafe {
                    windows_sys::Win32::Media::timeGetDevCaps(&mut caps, size_of_val(&caps) as u32)
                };
                if code != 0 {
                    return Err(format!("timeGetDevCaps failed ({code})").into());
                }
                if !(caps.wPeriodMin..=caps.wPeriodMax).contains(&period_ms) {
                    return Err(format!(
                        "WinMM supports {}–{} ms",
                        caps.wPeriodMin, caps.wPeriodMax
                    )
                    .into());
                }
                Self::WinMm { period_ms }
            }
        };
        Ok(request)
    }
}

impl fmt::Display for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("No request"),
            Self::Native { interval_100ns } => write!(f, "{interval_100ns} ms"),
            Self::WinMm { period_ms } => write!(f, "{period_ms} ms (WinMM)"),
        }
    }
}

// Serialize FastTimer's own requests within one process, including benchmark passes.
static OWNED: AtomicBool = AtomicBool::new(false);

pub struct Lease {
    request: Request,
    pub reported_100ns: Option<Interval>,
    active: bool,
}

impl Lease {
    pub fn acquire(request: Request) -> Result<Self> {
        if OWNED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("A timer measurement or request is already active in this process".into());
        }
        match change(request, true) {
            Ok(reported_100ns) => Ok(Self {
                request,
                reported_100ns,
                active: true,
            }),
            Err(error) => {
                OWNED.store(false, Ordering::Release);
                Err(error)
            }
        }
    }

    pub fn request(&self) -> Request {
        self.request
    }

    pub fn release(&mut self) -> Result<()> {
        if self.active {
            change(self.request, false)?;
            self.active = false;
            OWNED.store(false, Ordering::Release);
        }
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

fn change(request: Request, set: bool) -> Result<Option<Interval>> {
    match request {
        Request::None => Ok(None),
        Request::Native { interval_100ns } => {
            let mut current = 0;
            // BOOLEAN is one byte in the native API.
            let status =
                unsafe { NtSetTimerResolution(interval_100ns.0, u8::from(set), &mut current) };
            if !set && status == STATUS_TIMER_RESOLUTION_NOT_SET {
                return Ok(None);
            }
            nt_status("NtSetTimerResolution", status)?;
            Ok(Some(Interval(current)))
        }
        Request::WinMm { period_ms } => {
            let code = if set {
                unsafe { timeBeginPeriod(period_ms) }
            } else {
                unsafe { timeEndPeriod(period_ms) }
            };
            if code != 0 {
                let name = if set {
                    "timeBeginPeriod"
                } else {
                    "timeEndPeriod"
                };
                return Err(format!("{name} failed ({code})").into());
            }
            Ok(None)
        }
    }
}

/// Ask Windows to honor this process's requests even when its window is hidden.
pub fn honor_background_requests() -> io::Result<()> {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        StateMask: 0,
    };
    // The policy applies to this process only and the input lives through the call.
    let ok = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            (&raw const state).cast(),
            size_of_val(&state) as u32,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn windows_build() -> Result<u32> {
    let mut version = OSVERSIONINFOW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    nt_status("RtlGetVersion", unsafe { RtlGetVersion(&mut version) })?;
    Ok(version.dwBuildNumber)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_intervals_are_exact() {
        assert_eq!("0.5100".parse(), Ok(Interval(5100)));
        assert_eq!("1.5".parse(), Ok(Interval(15_000)));
        assert_eq!("429496.7295".parse(), Ok(Interval(u32::MAX)));
        for invalid in [
            "0",
            "-1",
            "NaN",
            "0.12345",
            "429496.7296",
            "1e-3",
            "+1",
            "1..0",
        ] {
            assert!(invalid.parse::<Interval>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn request_slot_is_reusable_after_release() -> Result<()> {
        let mut first = Lease::acquire(Request::None)?;
        assert!(Lease::acquire(Request::None).is_err());
        first.release()?;
        let second = Lease::acquire(Request::None)?;
        drop(first);
        assert!(Lease::acquire(Request::None).is_err());
        drop(second);
        Lease::acquire(Request::None)?.release()?;
        let range = query()?;
        for case in [Case::Fastest, Case::WinMm(1), Case::Fastest] {
            let mut request = Lease::acquire(Request::resolve(case, range)?)?;
            request.release()?;
            request.release()?;
        }
        Ok(())
    }

    #[test]
    fn native_range_uses_platform_bounds() -> Result<()> {
        let range = Snapshot {
            finest_100ns: Interval(8000),
            coarsest_100ns: Interval(100_000),
            current_100ns: Interval(10_000),
        };
        assert_eq!(
            Request::resolve(Case::Fastest, range)?,
            Request::Native {
                interval_100ns: Interval(8000)
            }
        );
        assert!(Request::resolve(Case::Native(Interval(7999)), range).is_err());
        assert!(Request::resolve(Case::Native(Interval(100_001)), range).is_err());
        Ok(())
    }
}
