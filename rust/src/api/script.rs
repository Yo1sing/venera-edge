use anyhow::Result;

/// A single Rust-owned QuickJS worker. No JS values or pointers cross FRB.
#[flutter_rust_bridge::frb(opaque)]
pub struct ScriptRuntime {
    pub(crate) inner: std::sync::Arc<crate::script::Engine>,
}
impl ScriptRuntime {
    pub fn open(options_json: String) -> Result<Self> {
        Ok(Self {
            inner: std::sync::Arc::new(crate::script::Engine::open(serde_json::from_str(
                &options_json,
            )?)?),
        })
    }
    pub fn configure(&self, options_json: String) -> Result<()> {
        self.inner.configure(serde_json::from_str(&options_json)?)
    }
    pub fn parse_source(&self, code: String) -> Result<String> {
        self.inner.evaluate(&format!(
            "__sources.install({})",
            serde_json::to_string(&code)?
        ))
    }
    pub fn initialize_source(&self, key: String) -> Result<()> {
        self.inner
            .evaluate(&format!("__sources.init({})", serde_json::to_string(&key)?))?;
        Ok(())
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn evaluate(&self, source: String) -> Result<String> {
        self.inner.evaluate(&source)
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn invoke(&self, id: u64, arguments_json: String) -> Result<String> {
        let arguments: serde_json::Value = serde_json::from_str(&arguments_json)?;
        self.inner.query(format!("__venera.call({id},{arguments})"))
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn retain(&self, id: u64) -> Result<()> {
        self.inner
            .query(format!("(__venera.retain({id}),'null')"))?;
        Ok(())
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn release(&self, id: u64) -> Result<()> {
        self.inner
            .query(format!("(__venera.release({id}),'null')"))?;
        Ok(())
    }
    pub async fn await_promise(&self, id: u64) -> Result<String> {
        self.inner.promise(id).await
    }
    pub async fn next_event(&self) -> Option<String> {
        self.inner.events.lock().await.recv().await
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn resolve_event(&self, id: u64, value_json: String, error: Option<String>) -> Result<()> {
        self.inner.settle(
            id,
            crate::script::unpack(&serde_json::from_str(&value_json)?)?,
            error,
        )
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn close(&self) {
        self.inner.close();
    }
}

#[flutter_rust_bridge::frb(sync)]
pub fn load_source_data(directory: String, key: String) -> Result<String> {
    Ok(crate::script::source_data(&directory, &key)?.to_string())
}

pub fn source_files(directory: String) -> Result<Vec<String>> {
    let directory = std::path::Path::new(&directory).join("comic_source");
    std::fs::create_dir_all(&directory)?;
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|v| v == "js") {
            files.push(entry.path().to_string_lossy().into_owned());
        }
    }
    files.sort();
    Ok(files)
}
pub fn read_source_file(path: String) -> Result<String> {
    Ok(std::fs::read_to_string(path)?)
}
pub fn create_source_file(directory: String, name: String, code: String) -> Result<String> {
    use std::io::Write;
    let directory = std::path::Path::new(&directory).join("comic_source");
    std::fs::create_dir_all(&directory)?;
    let name = crate::archives::safe_relative(&name)?;
    anyhow::ensure!(
        name.components().count() == 1,
        "Source name must be a file name"
    );
    let stem = name
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("source");
    for index in 0..10000 {
        let name = if index == 0 {
            format!("{stem}.js")
        } else {
            format!("{stem}({index}).js")
        };
        let path = directory.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(code.as_bytes())?;
                return Ok(path.to_string_lossy().into_owned());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("Too many source files with the same name")
}
#[flutter_rust_bridge::frb(sync)]
pub fn update_source_data(
    directory: String,
    key: String,
    path: Vec<String>,
    value_json: Option<String>,
) -> Result<()> {
    crate::script::update_source(
        &directory,
        &key,
        &path,
        value_json.map(|v| serde_json::from_str(&v)).transpose()?,
    )
}
pub async fn compute_script(
    options_json: String,
    source: String,
    arguments_json: String,
) -> Result<String> {
    let value = crate::script::compute(
        serde_json::from_str(&options_json)?,
        source,
        crate::script::unpack(&serde_json::from_str(&arguments_json)?)?,
    )
    .await?;
    Ok(crate::script::pack(&value).to_string())
}
