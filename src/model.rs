use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub clipboard: bool,
    pub edge: bool,
    pub auto_hide: bool,
    pub size: u32,
    pub limit: usize,
    pub folders: Vec<PathBuf>,
}
impl Default for Settings {
    fn default() -> Self {
        let pictures = crate::platform::pictures_folder();
        Self {
            clipboard: false,
            edge: true,
            auto_hide: true,
            size: 180,
            limit: 30,
            folders: vec![pictures.join("Screenshots")],
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub video: bool,
    pub pinned: bool,
    #[serde(default)]
    pub fingerprint: u64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub settings: Settings,
    pub items: Vec<Item>,
    pub archived: Vec<Item>,
    #[serde(skip)]
    pub root: PathBuf,
}
impl Store {
    pub fn load(root: PathBuf) -> AppResult<Self> {
        fs::create_dir_all(root.join("media"))?;
        let mut store: Self = match fs::read(root.join("state.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e.into()),
        };
        store.root = root;
        store.settings.size = store.settings.size.clamp(140, 240);
        store.settings.limit = store.settings.limit.clamp(1, 200);
        store.items.retain(|i| i.path.is_file());
        store.archived.retain(|i| i.path.is_file());
        Ok(store)
    }
    pub fn save(&self) -> AppResult<()> {
        let tmp = self.root.join("state.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        // ReplaceFile/MoveFileEx gives an atomic replacement on Windows.
        crate::platform::replace_file(&tmp, &self.root.join("state.json"))?;
        Ok(())
    }
    pub fn import(&mut self, source: &Path) -> AppResult<String> {
        let video = is_video(source);
        if !video && !is_image(source) {
            return Err("対応していないファイル形式です".into());
        }
        let id = unique_id();
        let (width, height, path, fingerprint) = if video {
            let path = self.root.join("media").join(format!(
                "{id}.{}",
                source.extension().unwrap_or_default().to_string_lossy()
            ));
            fs::copy(source, &path)?;
            (0, 0, path, 0)
        } else {
            let img = read_image(source)?;
            let fingerprint = image_fingerprint(&img);
            if let Some(existing) = self.items.iter().find(|i| i.fingerprint == fingerprint) {
                return Ok(existing.id.clone());
            }
            let path = self.root.join("media").join(format!("{id}.png"));
            img.save(&path)?;
            (img.width(), img.height(), path, fingerprint)
        };
        self.items.push(Item {
            id: id.clone(),
            path,
            name: source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            width,
            height,
            video,
            pinned: false,
            fingerprint,
        });
        self.enforce_limit();
        self.save()?;
        Ok(id)
    }
    pub fn add_image(&mut self, img: &image::DynamicImage, name: &str) -> AppResult<String> {
        let fingerprint = image_fingerprint(img);
        if let Some(existing) = self.items.iter().find(|i| i.fingerprint == fingerprint) {
            return Ok(existing.id.clone());
        }
        let id = unique_id();
        let path = self.root.join("media").join(format!("{id}.png"));
        img.save(&path)?;
        self.items.push(Item {
            id: id.clone(),
            path,
            name: name.into(),
            width: img.width(),
            height: img.height(),
            video: false,
            pinned: false,
            fingerprint,
        });
        self.enforce_limit();
        self.save()?;
        Ok(id)
    }
    pub fn dismiss(&mut self, id: &str) {
        if let Some(index) = self.items.iter().position(|i| i.id == id) {
            self.archived.push(self.items.remove(index));
        }
    }
    pub fn restore(&mut self) {
        if let Some(item) = self.archived.pop() {
            self.items.push(item);
        }
    }
    fn enforce_limit(&mut self) {
        while self.items.len() > self.settings.limit {
            let Some(index) = self.items.iter().position(|i| !i.pinned) else {
                break;
            };
            self.archived.push(self.items.remove(index));
        }
    }
}

/// Match the same Snipping Tool capture arriving through both the clipboard and its folder.
pub fn image_fingerprint(img: &image::DynamicImage) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    img.width().hash(&mut hash);
    img.height().hash(&mut hash);
    img.to_rgba8().as_raw().hash(&mut hash);
    hash.finish()
}

pub fn unique_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}
pub fn is_video(path: &Path) -> bool {
    extension(path, &["mp4", "mov", "mkv", "webm"])
}
pub fn is_image(path: &Path) -> bool {
    extension(path, &["png", "jpg", "jpeg", "bmp", "gif", "webp"])
}
fn extension(path: &Path, choices: &[&str]) -> bool {
    path.extension()
        .is_some_and(|e| choices.contains(&e.to_string_lossy().to_ascii_lowercase().as_str()))
}
pub fn read_image(path: &Path) -> AppResult<image::DynamicImage> {
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(32768);
    limits.max_image_height = Some(32768);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode()?)
}
/// Polling watches only new, stable files. Existing screenshots are never imported in bulk.
pub struct FolderWatcher {
    seen: HashMap<PathBuf, (u64, SystemTime)>,
    pending: HashMap<PathBuf, (u64, SystemTime)>,
}
impl FolderWatcher {
    pub fn new(folders: &[PathBuf]) -> Self {
        Self {
            seen: scan(folders),
            pending: HashMap::new(),
        }
    }
    pub fn poll(&mut self, folders: &[PathBuf]) -> Vec<PathBuf> {
        let current = scan(folders);
        let mut ready = Vec::new();
        for (path, stamp) in &current {
            if self.seen.get(path) == Some(stamp) {
                continue;
            }
            if self.pending.get(path) == Some(stamp) && stamp.0 > 0 {
                ready.push(path.clone());
                self.seen.insert(path.clone(), *stamp);
            }
        }
        self.pending = current;
        ready.sort();
        ready
    }
    pub fn retry(&mut self, path: &Path) {
        self.seen.remove(path);
    }
}
fn scan(folders: &[PathBuf]) -> HashMap<PathBuf, (u64, SystemTime)> {
    let mut entries = HashMap::new();
    for folder in folders {
        if let Ok(files) = fs::read_dir(folder) {
            for entry in files.flatten() {
                let path = entry.path();
                if (is_image(&path) || is_video(&path))
                    && let Ok(m) = entry.metadata()
                    && m.is_file()
                {
                    entries.insert(path, (m.len(), m.modified().unwrap_or(UNIX_EPOCH)));
                }
            }
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn watcher_waits_for_stable_new_files() {
        let root = std::env::temp_dir().join(format!("snapline-test-{}", unique_id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("old.png"), b"old").unwrap();
        let folders = vec![root.clone()];
        let mut w = FolderWatcher::new(&folders);
        assert!(w.poll(&folders).is_empty());
        fs::write(root.join("new.png"), b"part").unwrap();
        assert!(w.poll(&folders).is_empty());
        fs::write(root.join("new.png"), b"complete").unwrap();
        assert!(w.poll(&folders).is_empty());
        assert_eq!(w.poll(&folders), vec![root.join("new.png")]);
        assert!(w.poll(&folders).is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn import_dismiss_restore_preserves_original_and_pins() {
        let root = std::env::temp_dir().join(format!("snapline-test-{}", unique_id()));
        let mut s = Store::load(root.clone()).unwrap();
        s.settings.limit = 1;
        let img = image::DynamicImage::new_rgb8(20, 10);
        let id = s.add_image(&img, "one").unwrap();
        s.items[0].pinned = true;
        s.add_image(&image::DynamicImage::new_rgb8(24, 12), "two")
            .unwrap();
        assert_eq!(s.items[0].id, id);
        assert_eq!(s.archived.len(), 1);
        s.dismiss(&id);
        assert!(s.items.is_empty());
        s.restore();
        assert_eq!(s.items[0].id, id);
        s.save().unwrap();
        assert_eq!(Store::load(root.clone()).unwrap().items.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn clipboard_and_file_copy_are_one_item() {
        let root = std::env::temp_dir().join(format!("snapline-test-{}", unique_id()));
        let mut s = Store::load(root.clone()).unwrap();
        let img = image::DynamicImage::new_rgb8(13, 17);
        let first = s.add_image(&img, "clipboard").unwrap();
        let source = root.join("capture.png");
        img.save(&source).unwrap();
        assert_eq!(s.import(&source).unwrap(), first);
        assert_eq!(s.items.len(), 1);
        assert!(source.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
