use std::collections::BTreeMap;

use windows::core::PCWSTR;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, IAudioClient3, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};

pub type AppResult<T> = Result<T, String>;

pub fn error_text(context: &str, error: windows::core::Error) -> String {
    format!(
        "{} failed: {} (0x{:08X})",
        context,
        error.message(),
        error.code().0 as u32
    )
}

pub struct ComApartment;

impl ComApartment {
    pub fn initialize() -> AppResult<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|error| error_text("COM initialisation", error))?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub fn create_enumerator() -> AppResult<IMMDeviceEnumerator> {
    unsafe {
        CoCreateInstance(
            &MMDeviceEnumerator,
            None::<&windows::core::IUnknown>,
            CLSCTX_ALL,
        )
    }
    .map_err(|error| error_text("MMDeviceEnumerator creation", error))
}

fn activate_client(device: &IMMDevice) -> AppResult<IAudioClient3> {
    unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|error| error_text("IAudioClient3 activation", error))
}

struct MixFormat(*mut WAVEFORMATEX);

impl MixFormat {
    fn load(client: &IAudioClient3) -> AppResult<Self> {
        unsafe { client.GetMixFormat() }
            .map(Self)
            .map_err(|error| error_text("GetMixFormat", error))
    }

    fn as_ptr(&self) -> *const WAVEFORMATEX {
        self.0
    }
}

impl Drop for MixFormat {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CoTaskMemFree(Some(self.0 as *const core::ffi::c_void)) };
        }
    }
}

unsafe fn read_properties(client: &IAudioClient3, format: *const WAVEFORMATEX) -> AppResult<Props> {
    let mut default_period = 0u32;
    let mut fundamental_period = 0u32;
    let mut minimum_period = 0u32;
    let mut maximum_period = 0u32;

    unsafe {
        client
            .GetSharedModeEnginePeriod(
                format,
                &mut default_period,
                &mut fundamental_period,
                &mut minimum_period,
                &mut maximum_period,
            )
            .map_err(|error| error_text("GetSharedModeEnginePeriod", error))?;

        let format = &*format;
        Ok(Props {
            default_period,
            fundamental_period,
            minimum_period,
            maximum_period,
            sample_rate: format.nSamplesPerSec,
            bits_per_sample: format.wBitsPerSample,
            channels: format.nChannels,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Props {
    pub default_period: u32,
    pub fundamental_period: u32,
    pub minimum_period: u32,
    pub maximum_period: u32,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub channels: u16,
}

impl Props {
    pub fn milliseconds(&self, frames: u32) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }

        1000.0 * f64::from(frames) / f64::from(self.sample_rate)
    }

    pub fn low_latency_period(&self) -> Option<u32> {
        (self.minimum_period > 0 && self.minimum_period < self.default_period)
            .then_some(self.minimum_period)
    }
}

pub struct Activation {
    client: IAudioClient3,
    format: MixFormat,
    properties: Props,
}

impl Activation {
    pub fn properties(&self) -> &Props {
        &self.properties
    }
}

pub fn query(device: &IMMDevice) -> AppResult<Activation> {
    let client = activate_client(device)?;
    let format = MixFormat::load(&client)?;
    let properties = unsafe { read_properties(&client, format.as_ptr()) }?;
    Ok(Activation {
        client,
        format,
        properties,
    })
}

pub struct Session {
    client: IAudioClient3,
    _format: MixFormat,
    requested_period: u32,
}

impl Session {
    pub fn start(activation: Activation) -> AppResult<Self> {
        let Activation {
            client,
            format,
            properties,
        } = activation;

        let period = properties
            .low_latency_period()
            .ok_or("the driver does not expose a lower shared-mode period")?;

        unsafe { client.InitializeSharedAudioStream(0, period, format.as_ptr(), None) }
            .map_err(|error| error_text("InitializeSharedAudioStream", error))?;

        unsafe { client.Start() }.map_err(|error| error_text("IAudioClient::Start", error))?;

        Ok(Self {
            client,
            _format: format,
            requested_period: period,
        })
    }

    pub fn requested_period(&self) -> u32 {
        self.requested_period
    }

    pub fn current_period(&self) -> AppResult<u32> {
        let mut format: *mut WAVEFORMATEX = std::ptr::null_mut();
        let mut period = 0u32;

        let result = unsafe {
            self.client
                .GetCurrentSharedModeEnginePeriod(&mut format, &mut period)
        };

        if !format.is_null() {
            unsafe { CoTaskMemFree(Some(format as *const core::ffi::c_void)) };
        }

        result.map_err(|error| error_text("GetCurrentSharedModeEnginePeriod", error))?;
        Ok(period)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

pub fn device_id(device: &IMMDevice) -> AppResult<String> {
    let id = unsafe { device.GetId() }.map_err(|error| error_text("IMMDevice::GetId", error))?;

    let text = unsafe { id.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(id.0 as *const core::ffi::c_void)) };
    Ok(text)
}

pub fn friendly_name(device: &IMMDevice) -> String {
    const UNKNOWN: &str = "Unknown audio device";

    unsafe {
        let Ok(store) = device.OpenPropertyStore(STGM_READ) else {
            return UNKNOWN.into();
        };

        let Ok(mut value) = store.GetValue(&PKEY_Device_FriendlyName) else {
            return UNKNOWN.into();
        };

        let name = PropVariantToStringAlloc(&value)
            .ok()
            .and_then(|text| {
                let result = text.to_string().ok();
                CoTaskMemFree(Some(text.0 as *const core::ffi::c_void));
                result
            })
            .unwrap_or_else(|| UNKNOWN.into());

        let _ = PropVariantClear(&mut value);
        name
    }
}

#[derive(Clone, Debug)]
pub struct Target {
    pub id: String,
    pub role: &'static str,
    pub capture: bool,
}

fn add_default_target(
    enumerator: &IMMDeviceEnumerator,
    targets: &mut BTreeMap<String, Target>,
    capture: bool,
    role: windows::Win32::Media::Audio::ERole,
    role_name: &'static str,
) {
    let flow = if capture { eCapture } else { eRender };
    let Ok(device) = (unsafe { enumerator.GetDefaultAudioEndpoint(flow, role) }) else {
        return;
    };
    let Ok(id) = device_id(&device) else {
        return;
    };
    targets.entry(id.clone()).or_insert(Target {
        id,
        role: role_name,
        capture,
    });
}

pub fn collect_targets(enumerator: &IMMDeviceEnumerator) -> Vec<Target> {
    let mut targets = BTreeMap::new();

    add_defaults(enumerator, &mut targets, false);
    add_endpoints(enumerator, &mut targets, eRender, false);

    add_defaults(enumerator, &mut targets, true);
    add_endpoints(enumerator, &mut targets, eCapture, true);

    targets.into_values().collect()
}

fn add_defaults(
    enumerator: &IMMDeviceEnumerator,
    targets: &mut BTreeMap<String, Target>,
    capture: bool,
) {
    add_default_target(enumerator, targets, capture, eConsole, "console");
    add_default_target(enumerator, targets, capture, eMultimedia, "multimedia");
    add_default_target(
        enumerator,
        targets,
        capture,
        eCommunications,
        "communications",
    );
}

fn add_endpoints(
    enumerator: &IMMDeviceEnumerator,
    targets: &mut BTreeMap<String, Target>,
    flow: EDataFlow,
    capture: bool,
) {
    let Ok(collection) = (unsafe { enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) })
    else {
        return;
    };

    let count = unsafe { collection.GetCount() }.unwrap_or(0);
    for index in 0..count {
        let Ok(device) = (unsafe { collection.Item(index) }) else {
            continue;
        };
        let Ok(id) = device_id(&device) else {
            continue;
        };
        targets.entry(id.clone()).or_insert(Target {
            id,
            role: "active",
            capture,
        });
    }
}

pub fn wide_null(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn pcwstr(text: &[u16]) -> PCWSTR {
    PCWSTR(text.as_ptr())
}
