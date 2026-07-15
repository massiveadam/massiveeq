use rustfft::{FftPlanner, num_complex::Complex64};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_long, c_void},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use thiserror::Error;

const SFM_READ: c_int = 0x10;
const SRC_SINC_BEST_QUALITY: c_int = 0;

#[repr(C)]
struct SfInfo {
    frames: i64,
    samplerate: c_int,
    channels: c_int,
    format: c_int,
    sections: c_int,
    seekable: c_int,
}

#[repr(C)]
struct SrcData {
    data_in: *const f32,
    data_out: *mut f32,
    input_frames: c_long,
    output_frames: c_long,
    input_frames_used: c_long,
    output_frames_gen: c_long,
    end_of_input: c_int,
    src_ratio: f64,
}

#[link(name = "sndfile")]
unsafe extern "C" {
    fn sf_open(path: *const c_char, mode: c_int, info: *mut SfInfo) -> *mut c_void;
    fn sf_readf_float(handle: *mut c_void, samples: *mut f32, frames: i64) -> i64;
    fn sf_close(handle: *mut c_void) -> c_int;
    fn sf_strerror(handle: *mut c_void) -> *const c_char;
}

#[link(name = "samplerate")]
unsafe extern "C" {
    fn src_simple(data: *mut SrcData, converter_type: c_int, channels: c_int) -> c_int;
    fn src_strerror(error: c_int) -> *const c_char;
}

#[derive(Debug, Error)]
pub enum IrError {
    #[error("could not open impulse response {path}: {message}")]
    Open { path: PathBuf, message: String },
    #[error("could not decode impulse response {path}: {message}")]
    Decode { path: PathBuf, message: String },
    #[error("impulse response is empty")]
    Empty,
    #[error("impulse response is longer than 10 seconds")]
    TooLong,
    #[error("impulse response contains a non-finite sample")]
    NonFinite,
    #[error("invalid target sample rate {0}")]
    InvalidRate(u32),
    #[error("impulse response resampling failed: {0}")]
    Resample(String),
}

#[derive(Debug, Clone)]
pub struct IrData {
    pub source_rate: u32,
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl IrData {
    pub fn frames(&self) -> usize {
        self.channels.first().map(Vec::len).unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct ConvolutionKernel {
    pub sample_rate: u32,
    pub impulse: Vec<f32>,
    pub latency_frames: u32,
}

impl ConvolutionKernel {
    pub fn transfer_grid(&self, fft_len: usize) -> Vec<Complex64> {
        let length = fft_len.max(self.impulse.len()).next_power_of_two();
        let mut values = vec![Complex64::new(0.0, 0.0); length];
        for (value, sample) in values.iter_mut().zip(&self.impulse) {
            value.re = *sample as f64;
        }
        FftPlanner::<f64>::new()
            .plan_fft_forward(length)
            .process(&mut values);
        values
    }
}

pub fn load_ir(path: &Path, target_rate: u32) -> Result<IrData, IrError> {
    if !(8_000..=384_000).contains(&target_rate) {
        return Err(IrError::InvalidRate(target_rate));
    }
    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| IrError::Open {
        path: path.to_owned(),
        message: "path contains a NUL byte".into(),
    })?;
    let mut info = SfInfo {
        frames: 0,
        samplerate: 0,
        channels: 0,
        format: 0,
        sections: 0,
        seekable: 0,
    };
    // SAFETY: c_path and info are valid for the duration of the call.
    let handle = unsafe { sf_open(c_path.as_ptr(), SFM_READ, &mut info) };
    if handle.is_null() {
        return Err(IrError::Open {
            path: path.to_owned(),
            message: sndfile_error(handle),
        });
    }
    let result = (|| {
        if info.frames <= 0 || info.channels <= 0 || info.samplerate <= 0 {
            return Err(IrError::Empty);
        }
        if info.frames as f64 / info.samplerate as f64 > 10.0 {
            return Err(IrError::TooLong);
        }
        let channels = info.channels as usize;
        let mut interleaved = vec![0.0_f32; info.frames as usize * channels];
        // SAFETY: the buffer is sized for the requested number of interleaved frames.
        let read = unsafe { sf_readf_float(handle, interleaved.as_mut_ptr(), info.frames) };
        if read != info.frames {
            return Err(IrError::Decode {
                path: path.to_owned(),
                message: sndfile_error(handle),
            });
        }
        if !interleaved.iter().all(|value| value.is_finite()) {
            return Err(IrError::NonFinite);
        }
        let resampled = if info.samplerate as u32 == target_rate {
            interleaved
        } else {
            resample_interleaved(&interleaved, channels, info.samplerate as u32, target_rate)?
        };
        let mut planar = vec![Vec::with_capacity(resampled.len() / channels); channels];
        for frame in resampled.chunks_exact(channels) {
            for (channel, sample) in planar.iter_mut().zip(frame) {
                channel.push(*sample);
            }
        }
        Ok(IrData {
            source_rate: info.samplerate as u32,
            sample_rate: target_rate,
            channels: planar,
        })
    })();
    // SAFETY: handle came from sf_open and has not been closed.
    let _ = unsafe { sf_close(handle) };
    result
}

fn resample_interleaved(
    input: &[f32],
    channels: usize,
    source_rate: u32,
    target_rate: u32,
) -> Result<Vec<f32>, IrError> {
    let input_frames = input.len() / channels;
    let ratio = target_rate as f64 / source_rate as f64;
    let output_frames = ((input_frames as f64 * ratio).ceil() as usize + 256).max(1);
    let mut output = vec![0.0_f32; output_frames * channels];
    let mut data = SrcData {
        data_in: input.as_ptr(),
        data_out: output.as_mut_ptr(),
        input_frames: input_frames as c_long,
        output_frames: output_frames as c_long,
        input_frames_used: 0,
        output_frames_gen: 0,
        end_of_input: 1,
        src_ratio: ratio,
    };
    // SAFETY: input/output buffers and frame counts agree and remain live for the call.
    let error = unsafe { src_simple(&mut data, SRC_SINC_BEST_QUALITY, channels as c_int) };
    if error != 0 {
        // SAFETY: libsamplerate returns a static NUL-terminated error string.
        let message = unsafe { CStr::from_ptr(src_strerror(error)) }
            .to_string_lossy()
            .into_owned();
        return Err(IrError::Resample(message));
    }
    output.truncate(data.output_frames_gen as usize * channels);
    Ok(output)
}

fn sndfile_error(handle: *mut c_void) -> String {
    // SAFETY: libsndfile permits a null handle here and returns a static string.
    let pointer = unsafe { sf_strerror(handle) };
    if pointer.is_null() {
        "unknown libsndfile error".into()
    } else {
        // SAFETY: pointer is a NUL-terminated static error string.
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }
}
