use anyhow::{Context, Result};
use std::{fs, io::Write, path::PathBuf};

#[derive(Debug, Clone)]
pub struct PlaylistSummary {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct PlaylistStore {
    dir: PathBuf,
}

impl PlaylistStore {
    pub fn new(data_dir: PathBuf) -> Result<Self> {
        let dir = data_dir.join("playlists");
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn list(&self) -> Result<Vec<PlaylistSummary>> {
        let mut playlists = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("m3u") {
                let Some(name) = path.file_stem().and_then(|s| s.to_str()) else { continue; };
                playlists.push(PlaylistSummary { name: name.to_string() });
            }
        }
        playlists.sort_by_key(|p| p.name.to_lowercase());
        Ok(playlists)
    }

    pub fn create(&self, name: &str) -> Result<()> {
        let path = self.path_for(name);
        if path.exists() {
            anyhow::bail!("playlist already exists: {name}");
        }
        let mut file = fs::File::create(path)?;
        writeln!(file, "#EXTM3U")?;
        Ok(())
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        fs::remove_file(self.path_for(name)).with_context(|| format!("failed to delete playlist {name}"))
    }

    pub fn read(&self, name: &str) -> Result<Vec<PathBuf>> {
        let path = self.path_for(name);
        let content = fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
        Ok(content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(PathBuf::from)
            .collect())
    }

    pub fn append(&self, name: &str, files: &[PathBuf]) -> Result<()> {
        let path = self.path_for(name);
        if !path.exists() {
            self.create(name)?;
        }
        let mut existing = self.read(name)?;
        existing.extend(files.iter().cloned());
        self.write(name, &existing)
    }

    pub fn write(&self, name: &str, files: &[PathBuf]) -> Result<()> {
        let mut out = String::from("#EXTM3U\n");
        for file in files {
            out.push_str(&file.to_string_lossy());
            out.push('\n');
        }
        fs::write(self.path_for(name), out)?;
        Ok(())
    }

    pub fn file_path(&self, name: &str) -> PathBuf {
        self.path_for(name)
    }

    fn path_for(&self, name: &str) -> PathBuf {
        let safe = name.trim().trim_end_matches(".m3u");
        self.dir.join(format!("{safe}.m3u"))
    }
}
