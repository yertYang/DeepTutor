//! Local OCR. Apple Vision and CoreGraphics run in-process; no shell, model API
//! or interpreter is required by the packaged application.
use crate::{Error, Result};
static STOPPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn reset() {
    STOPPING.store(false, std::sync::atomic::Ordering::Release);
}
pub(crate) fn stop() {
    STOPPING.store(true, std::sync::atomic::Ordering::Release);
}
pub const fn available() -> bool {
    cfg!(target_os = "macos")
}
#[cfg(not(target_os = "macos"))]
pub fn image(_bytes: &[u8]) -> Result<String> {
    Err(Error::unsupported(
        "Local image OCR is currently available on macOS. This platform has no configured OCR engine.",
    ))
}
#[cfg(not(target_os = "macos"))]
pub fn pdf(_bytes: &[u8]) -> Result<String> {
    Err(Error::unsupported(
        "This PDF has no extractable text. Local scanned-PDF OCR is currently available on macOS.",
    ))
}
#[cfg(target_os = "macos")]
mod apple {
    use super::*;
    use objc2::{AnyThread, rc::autoreleasepool};
    use objc2_core_foundation::{CFData, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGContext, CGDataProvider,
        CGPDFBox, CGPDFDocument, CGPDFPage,
    };
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
    };
    static ACTIVE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn guard() -> Result<std::sync::MutexGuard<'static, ()>> {
        match ACTIVE.try_lock() {
            Ok(lock) => Ok(lock),
            Err(std::sync::TryLockError::Poisoned(error)) => Ok(error.into_inner()),
            Err(_) => Err(Error::conflict(
                "Another OCR import is running. Retry when it finishes.",
            )),
        }
    }
    fn recognize(handler: &VNImageRequestHandler) -> Result<String> {
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(false);
        #[allow(deprecated)]
        // SAFETY: CPU-only execution is supported by this request and avoids GPU service dependencies in a headless server.
        unsafe {
            request.setUsesCPUOnly(true);
        }
        // SAFETY: This is a configured recognition request. No pointers or
        // completion callbacks are supplied, and the synchronous call owns it.
        let supported = unsafe { request.supportedRecognitionLanguagesAndReturnError() }
            .map_err(|_| Error::invalid("The local OCR language models are unavailable."))?;
        let mut languages = Vec::new();
        for language in ["zh-Hans", "en-US"] {
            if supported.iter().any(|value| value.to_string() == language) {
                languages.push(NSString::from_str(language));
            }
        }
        if !languages.is_empty() {
            request.setRecognitionLanguages(&NSArray::from_retained_slice(&languages));
        }
        let base: &VNRequest = &request;
        handler
            .performRequests_error(&NSArray::from_slice(&[base]))
            .map_err(|_| Error::invalid("The local OCR engine could not recognize this image."))?;
        let mut text = String::new();
        if let Some(results) = request.results() {
            for observation in results.iter() {
                if let Some(candidate) = observation.topCandidates(1).firstObject() {
                    let line = candidate.string().to_string();
                    if text.len() + line.len() + 1 > crate::knowledge::MAX_FILE {
                        return Err(Error::invalid("OCR text exceeds 2 MiB."));
                    }
                    text.push_str(&line);
                    text.push('\n');
                }
            }
        }
        Ok(text)
    }
    pub fn image(bytes: &[u8]) -> Result<String> {
        let dimensions = imagesize::blob_size(bytes)
            .map_err(|_| Error::invalid("Invalid PNG or JPEG image."))?;
        if dimensions.width == 0
            || dimensions.height == 0
            || dimensions.width > 12000
            || dimensions.height > 12000
            || dimensions
                .width
                .checked_mul(dimensions.height)
                .is_none_or(|pixels| pixels > 12_000_000)
        {
            return Err(Error::invalid(
                "OCR images must contain at most 12 million pixels and be at most 12000 pixels per side.",
            ));
        }
        let _guard = guard()?;
        autoreleasepool(|_| {
            let data = NSData::with_bytes(bytes);
            let handler = VNImageRequestHandler::initWithData_options(
                VNImageRequestHandler::alloc(),
                &data,
                &NSDictionary::new(),
            );
            let text = recognize(&handler)?;
            if text.trim().is_empty() {
                return Err(Error::invalid(
                    "No recognizable text was found in the image.",
                ));
            }
            Ok(format!("[OCR]\n{text}"))
        })
    }
    pub fn pdf(bytes: &[u8]) -> Result<String> {
        let _guard = guard()?;
        let data = CFData::from_bytes(bytes);
        let provider = CGDataProvider::with_cf_data(Some(&data))
            .ok_or_else(|| Error::invalid("Invalid PDF image provider."))?;
        let document = CGPDFDocument::with_provider(Some(&provider))
            .ok_or_else(|| Error::invalid("Invalid scanned PDF."))?;
        let count = CGPDFDocument::number_of_pages(Some(&document));
        if count == 0 || count > 50 {
            return Err(Error::invalid(
                "Scanned PDF OCR supports 1-50 pages. Split larger files before importing.",
            ));
        }
        let mut text = String::new();
        let mut nonempty = false;
        for number in 1..=count {
            let page = CGPDFDocument::page(Some(&document), number)
                .ok_or_else(|| Error::invalid("Invalid PDF page."))?;
            let bounds = CGPDFPage::box_rect(Some(&page), CGPDFBox::MediaBox);
            let (w, h) = (bounds.size.width, bounds.size.height);
            if !w.is_finite()
                || !h.is_finite()
                || w <= 0.0
                || h <= 0.0
                || w > 20000.0
                || h > 20000.0
            {
                return Err(Error::invalid("PDF page dimensions exceed OCR limits."));
            }
            let scale = 2.5_f64
                .min((12_000_000.0 / (w * h)).sqrt())
                .min(12000.0 / w.max(h));
            let width = (w * scale).floor().max(1.0) as usize;
            let height = (h * scale).floor().max(1.0) as usize;
            let page_text = autoreleasepool(|_| {
                let color = CGColorSpace::new_device_rgb()
                    .ok_or_else(|| Error::invalid("OCR color space is unavailable."))?;
                // SAFETY: A null data pointer asks CoreGraphics to own the bounded
                // pixel buffer (<=48 MiB). RGB uses four bytes/pixel, skip-last alpha.
                let context = unsafe {
                    CGBitmapContextCreate(
                        std::ptr::null_mut(),
                        width,
                        height,
                        8,
                        width * 4,
                        Some(&color),
                        5,
                    )
                }
                .ok_or_else(|| Error::invalid("Could not render the scanned PDF page."))?;
                let rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: width as f64,
                        height: height as f64,
                    },
                };
                CGContext::set_rgb_fill_color(Some(&context), 1.0, 1.0, 1.0, 1.0);
                CGContext::fill_rect(Some(&context), rect);
                CGContext::concat_ctm(
                    Some(&context),
                    CGPDFPage::drawing_transform(Some(&page), CGPDFBox::MediaBox, rect, 0, true),
                );
                CGContext::draw_pdf_page(Some(&context), Some(&page));
                let image = CGBitmapContextCreateImage(Some(&context))
                    .ok_or_else(|| Error::invalid("Could not read the rendered PDF page."))?;
                // SAFETY: The image is retained for the complete synchronous
                // request; an empty options dictionary has no typed values.
                let handler = unsafe {
                    VNImageRequestHandler::initWithCGImage_options(
                        VNImageRequestHandler::alloc(),
                        &image,
                        &NSDictionary::new(),
                    )
                };
                recognize(&handler)
            })?;
            nonempty |= !page_text.trim().is_empty();
            let heading = format!("\n[Page {number}; OCR]\n");
            if text.len() + heading.len() + page_text.len() > crate::knowledge::MAX_FILE {
                return Err(Error::invalid("OCR text exceeds 2 MiB."));
            }
            text.push_str(&heading);
            text.push_str(&page_text);
        }
        if !nonempty {
            return Err(Error::invalid(
                "No recognizable text was found in the scanned PDF.",
            ));
        }
        Ok(text)
    }
}
#[cfg(all(target_os = "macos", not(test)))]
fn isolated(bytes: &[u8], format: &str) -> Result<String> {
    use std::{
        io::{Read, Write},
        process::{Command, Stdio},
        sync::{Mutex, TryLockError},
        time::{Duration, Instant},
    };
    static WORKER: Mutex<()> = Mutex::new(());
    if STOPPING.load(std::sync::atomic::Ordering::Acquire) {
        return Err(Error::conflict(
            "OCR import cancelled because the server is stopping.",
        ));
    }
    let _guard = match WORKER.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(error)) => error.into_inner(),
        Err(_) => {
            return Err(Error::conflict(
                "Another OCR import is running. Retry when it finishes.",
            ));
        }
    };
    if bytes.len() > crate::document_parser::MAX_UPLOAD {
        return Err(Error::invalid("OCR upload exceeds 16 MiB."));
    }
    let mut input = tempfile::NamedTempFile::new()?;
    input.write_all(bytes)?;
    input.flush()?;
    let output = tempfile::NamedTempFile::new()?;
    struct Worker(std::process::Child);
    impl Drop for Worker {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    let child = Command::new(std::env::current_exe()?)
        .arg("internal-ocr")
        .arg("--input")
        .arg(input.path())
        .arg("--output")
        .arg(output.path())
        .arg("--format")
        .arg(format)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut child = Worker(child);
    let started = Instant::now();
    loop {
        if STOPPING.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::conflict(
                "OCR import cancelled because the server is stopping.",
            ));
        }
        if let Some(status) = child.0.try_wait()? {
            if !status.success() {
                return Err(Error::invalid(
                    "Local OCR could not recognize this document. Check the original image and try a smaller file.",
                ));
            }
            break;
        }
        if started.elapsed() > Duration::from_secs(180) {
            return Err(Error::invalid(
                "Local OCR timed out after 180 seconds. Split the document and retry.",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut text = String::new();
    std::fs::File::open(output.path())?
        .take(crate::knowledge::MAX_FILE as u64 + 1)
        .read_to_string(&mut text)?;
    if text.len() > crate::knowledge::MAX_FILE || text.trim().is_empty() {
        return Err(Error::invalid("Invalid OCR output."));
    }
    Ok(text)
}
#[cfg(all(target_os = "macos", not(test)))]
pub fn image(bytes: &[u8]) -> Result<String> {
    isolated(bytes, "image")
}
#[cfg(all(target_os = "macos", not(test)))]
pub fn pdf(bytes: &[u8]) -> Result<String> {
    isolated(bytes, "pdf")
}
#[cfg(all(target_os = "macos", test))]
pub use apple::{image, pdf};
/// Hidden worker entry point. It never opens a runtime home or its credentials.
pub fn worker(input: &std::path::Path, output: &std::path::Path, format: &str) -> Result<()> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(input)?
        .take(crate::document_parser::MAX_UPLOAD as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > crate::document_parser::MAX_UPLOAD {
        return Err(Error::invalid("OCR upload exceeds 16 MiB."));
    }
    #[cfg(target_os = "macos")]
    {
        let text = match format {
            "image" => apple::image(&bytes)?,
            "pdf" => apple::pdf(&bytes)?,
            _ => return Err(Error::invalid("Unknown OCR input format.")),
        };
        std::fs::write(output, text)?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (format, bytes, output);
        Err(Error::unsupported(
            "No local OCR engine is available on this platform.",
        ))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    #[test]
    fn malformed_and_oversized_images_are_rejected_before_starting_vision() {
        let mut oversized = include_bytes!("../tests/fixtures/ocr.png").to_vec();
        oversized[16..20].copy_from_slice(&13000u32.to_be_bytes());
        assert!(image(&oversized).is_err());
        assert!(image(b"invalid").is_err());
    }
}
