//! Windows Imaging Component decoding: HEIC/HEIF, AVIF, camera RAW and anything else Windows
//! itself can open (including codecs from the Microsoft Store image extensions).

use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory, WICBitmapInterpolationModeFant,
    WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

/// Decode to RGBA, scaled to fit `max_px`. Returns (pixels, width, height, original dims).
pub fn decode(bytes: &[u8], max_px: u32) -> Result<(Vec<u8>, u32, u32, [u32; 2]), String> {
    let e = |x: windows::core::Error| x.message().to_string();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(e)?;
        let stream = f.CreateStream().map_err(e)?;
        stream.InitializeFromMemory(bytes).map_err(e)?;
        let dec = f.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).map_err(e)?;
        let frame = dec.GetFrame(0).map_err(e)?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).map_err(e)?;
        if w == 0 || h == 0 {
            return Err("empty image".into());
        }
        let scale = (max_px as f64 / w.max(h) as f64).min(1.0);
        let nw = ((w as f64 * scale).round() as u32).max(1);
        let nh = ((h as f64 * scale).round() as u32).max(1);
        let scaler = f.CreateBitmapScaler().map_err(e)?;
        scaler.Initialize(&frame, nw, nh, WICBitmapInterpolationModeFant).map_err(e)?;
        let conv = WICConvertBitmapSource(&GUID_WICPixelFormat32bppRGBA, &scaler).map_err(e)?;
        let mut buf = vec![0u8; (nw * nh * 4) as usize];
        conv.CopyPixels(std::ptr::null(), nw * 4, &mut buf).map_err(e)?;
        Ok((buf, nw, nh, [w, h]))
    }
}
