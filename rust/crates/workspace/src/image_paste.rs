//! An image on the clipboard pasted into a terminal whose programs run on another machine: it goes there as a
//! PNG file and the terminal gets that file's path, which an agent reads as an attached image.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

/// Keeps PNG bytes as a file where the terminal's programs run and answers its path there, or what to tell.
pub type ImageUpload = Arc<dyn Fn(&[u8]) -> Result<String, String> + Send + Sync>;

pub struct ClipboardImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The clipboard's image when it holds one and no text; text pastes as it always did.
pub fn clipboard_image() -> Option<ClipboardImage> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    if clipboard.get_text().is_ok_and(|text| !text.is_empty()) {
        return None;
    }
    let image = clipboard.get_image().ok()?;
    Some(ClipboardImage {
        width: u32::try_from(image.width).ok()?,
        height: u32::try_from(image.height).ok()?,
        rgba: image.bytes.into_owned(),
    })
}

pub fn encode_png(image: &ClipboardImage) -> Result<Vec<u8>, String> {
    let buffer = image::RgbaImage::from_raw(image.width, image.height, image.rgba.clone())
        .ok_or_else(|| "the clipboard's image is not whole".to_string())?;
    let mut png = std::io::Cursor::new(Vec::new());
    buffer
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(png.into_inner())
}

/// Uploads running off the UI thread, each for the item that asked, keyed by `K` (where that item is).
pub struct ImagePastes<K> {
    upload: ImageUpload,
    sender: Sender<(K, String, Result<String, String>)>,
    receiver: Receiver<(K, String, Result<String, String>)>,
}

impl<K: Send + 'static> ImagePastes<K> {
    pub fn new(upload: ImageUpload) -> Self {
        let (sender, receiver) = mpsc::channel();
        ImagePastes {
            upload,
            sender,
            receiver,
        }
    }

    pub fn start(&self, place: K, item: String, image: ClipboardImage) {
        let upload = self.upload.clone();
        let sender = self.sender.clone();
        let spawned = std::thread::Builder::new()
            .name("image-paste".into())
            .spawn(move || {
                let result = encode_png(&image).and_then(|png| upload(&png));
                if sender.send((place, item, result)).is_err() {
                    eprintln!("image paste: the window closed before the upload finished");
                }
                ui::wake();
            });
        if let Err(error) = spawned {
            eprintln!("image paste: {error}");
        }
    }

    /// The uploads that finished: where the item is, its id, and the path or why not.
    pub fn take_done(&self) -> Vec<(K, String, Result<String, String>)> {
        self.receiver.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clipboard_image_encodes_as_a_png_of_its_size() -> Result<(), String> {
        let image = ClipboardImage {
            width: 3,
            height: 2,
            rgba: (0..24).collect(),
        };
        let png = encode_png(&image)?;
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .map_err(|error| error.to_string())?
            .to_rgba8();
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
        assert_eq!(decoded.into_raw(), image.rgba);
        Ok(())
    }

    #[test]
    fn an_image_shorter_than_its_size_is_refused() {
        let image = ClipboardImage {
            width: 10,
            height: 10,
            rgba: vec![0; 12],
        };
        assert!(encode_png(&image).is_err());
    }

    #[test]
    fn an_upload_reports_the_path_for_the_item_that_asked() {
        let pastes: ImagePastes<u8> =
            ImagePastes::new(Arc::new(|png: &[u8]| Ok(format!("/tmp/{}.png", png.len()))));
        let image = ClipboardImage {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 4],
        };
        pastes.start(7, "terminal:1".into(), image);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut done = Vec::new();
        while done.is_empty() && std::time::Instant::now() < deadline {
            done = pastes.take_done();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let [(place, item, Ok(path))] = done.as_slice() else {
            panic!("no path: {done:?}");
        };
        assert_eq!((*place, item.as_str()), (7, "terminal:1"));
        assert!(path.starts_with("/tmp/") && path.ends_with(".png"));
    }
}
