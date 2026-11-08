//! Switches for Windows itself: dark mode, mute, Bluetooth and Wi-Fi. Each changes only what it
//! says, the same setting Windows' own quick settings change, and says what it did.

use crate::util::SendHwnd;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD, RegGetValueW, RegSetKeyValueW};
use windows::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, PostMessageW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE};
use windows::core::w;

const PERSONALIZE: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

fn read_dword(value: windows::core::PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = 4u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, PERSONALIZE, value, RRF_RT_REG_DWORD, None, Some(&mut data as *mut _ as *mut _), Some(&mut size)) }.is_ok().then_some(data)
}

fn write_dword(value: windows::core::PCWSTR, data: u32) -> bool {
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, PERSONALIZE, value, REG_DWORD.0, Some(&data as *const _ as *const _), 4) }.is_ok()
}

/// Switches apps and Windows between dark and light, as Settings → Personalisation → Colours does.
pub fn toggle_dark_mode() -> Result<String, String> {
    let light = read_dword(w!("AppsUseLightTheme")).unwrap_or(1) != 0;
    let next = if light { 0 } else { 1 };
    if !write_dword(w!("AppsUseLightTheme"), next) || !write_dword(w!("SystemUsesLightTheme"), next) {
        return Err("Windows didn't let Orbit change the colour mode.".into());
    }
    // Tells open windows, the taskbar included, to look again. Hung ones are skipped.
    let topic = w!("ImmersiveColorSet");
    unsafe {
        let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(topic.as_ptr() as isize), SMTO_ABORTIFHUNG, 200, None);
    }
    Ok(if light { "Dark mode on".into() } else { "Light mode on".into() })
}

/// Mutes or unmutes the default speakers.
pub fn toggle_mute() -> Result<String, String> {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender};
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    let failed = |_| "Windows didn't let Orbit reach the sound settings.".to_string();
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(failed)?;
        let speakers = devices.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|_| "There's no sound output to mute.".to_string())?;
        let volume: IAudioEndpointVolume = speakers.Activate(CLSCTX_ALL, None).map_err(failed)?;
        let muted = volume.GetMute().map_err(failed)?.as_bool();
        volume.SetMute(!muted, std::ptr::null()).map_err(failed)?;
        Ok(if muted { "Sound on".into() } else { "Sound muted".into() })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Radio {
    Bluetooth,
    WiFi,
}

impl Radio {
    fn name(self) -> &'static str {
        match self {
            Radio::Bluetooth => "Bluetooth",
            Radio::WiFi => "Wi-Fi",
        }
    }
}

/// Turns a radio on or off, on a thread of its own, since Windows answers in its own time. The
/// outcome comes back as text in a posted `message`.
pub fn toggle_radio(radio: Radio, main: SendHwnd, message: u32) {
    let _ = std::thread::Builder::new().name("orbit-radio".into()).spawn(move || {
        let outcome = switch_radio(radio).unwrap_or_else(|why| why);
        let pointer = crate::util::box_for_post(outcome);
        unsafe {
            if PostMessageW(Some(HWND(main.0 as *mut _)), message, WPARAM(pointer), LPARAM(0)).is_err() {
                drop(crate::util::take_posted::<String>(pointer));
            }
        }
    });
}

fn switch_radio(radio: Radio) -> Result<String, String> {
    use windows::Devices::Radios::{Radio as WinRadio, RadioAccessStatus, RadioKind, RadioState};
    let failed = |_| format!("Windows didn't let Orbit change {}.", radio.name());
    let access = WinRadio::RequestAccessAsync().and_then(|asked| asked.join()).map_err(failed)?;
    if access != RadioAccessStatus::Allowed {
        return Err(format!("Windows didn't let Orbit change {}. Check Settings → Privacy & security → Radios.", radio.name()));
    }
    let kind = match radio {
        Radio::Bluetooth => RadioKind::Bluetooth,
        Radio::WiFi => RadioKind::WiFi,
    };
    let radios = WinRadio::GetRadiosAsync().and_then(|asked| asked.join()).map_err(failed)?;
    let found = radios.into_iter().find(|each| each.Kind().is_ok_and(|each_kind| each_kind == kind));
    let Some(found) = found else { return Err(format!("This PC has no {} to turn on or off.", radio.name())) };
    let on = found.State().map_err(failed)? == RadioState::On;
    let next = if on { RadioState::Off } else { RadioState::On };
    let status = found.SetStateAsync(next).and_then(|asked| asked.join()).map_err(failed)?;
    if status != RadioAccessStatus::Allowed {
        return Err(format!("Windows didn't let Orbit change {}.", radio.name()));
    }
    Ok(format!("{} {}", radio.name(), if on { "off" } else { "on" }))
}
