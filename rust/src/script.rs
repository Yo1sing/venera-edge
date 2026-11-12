use anyhow::{anyhow, ensure, Context as _, Result};
use rquickjs::{Context, Ctx, FromJs, Function, Runtime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::HashMap,
    io::Write,
    path::PathBuf,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Options {
    #[serde(default)]
    pub worker_depth: u32,
    pub data_path: String,
    pub version: String,
    pub locale: String,
    pub platform: String,
    pub user_agent: String,
    #[serde(default)]
    pub cloudflare_user_agent: String,
    pub http: crate::api::http::HttpOptions,
}

type Reply = mpsc::SyncSender<Result<String>>;
enum Command {
    Eval(String, Reply),
    Configure(Box<Options>, Reply),
    Settle(u64, Result<Value>),
    Wake,
}

pub struct Engine {
    commands: mpsc::Sender<Command>,
    closed: Arc<AtomicBool>,
    pub events: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<String>>,
}

pub(crate) fn executor() -> &'static tokio::runtime::Runtime {
    static EXECUTOR: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    EXECUTOR.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .build()
            .expect("script executor")
    })
}

pub(crate) fn js_error(ctx: &Ctx<'_>, error: rquickjs::Error) -> anyhow::Error {
    if error.is_exception() {
        let value = ctx.catch();
        let stack = value
            .as_object()
            .and_then(|v| v.get::<_, String>("stack").ok());
        let message = rquickjs::Coerced::<String>::from_js(ctx, value)
            .map(|v| v.0)
            .unwrap_or_else(|_| "JavaScript exception".into());
        anyhow!("{message}\n{}", stack.unwrap_or_default())
    } else {
        anyhow!(error.to_string())
    }
}

fn evaluate(ctx: &Ctx<'_>, code: &str) -> Result<String> {
    ctx.eval::<String, _>(code).map_err(|e| js_error(ctx, e))
}

impl Engine {
    pub fn open(options: Options) -> Result<Self> {
        let (commands, incoming) = mpsc::channel();
        let (events, event_receiver) = tokio::sync::mpsc::channel(256);
        let closed = Arc::new(AtomicBool::new(false));
        let stop = closed.clone();
        let sender = commands.clone();
        let (ready, initialized) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("venera-quickjs".into())
            .spawn(move || {
                let result = (|| -> Result<()> {
                    let runtime = Runtime::new()?;
                    runtime.set_max_stack_size(2 * 1024 * 1024);
                    let deadline = Arc::new(Mutex::new(Instant::now() + Duration::from_secs(30)));
                    let interrupt = deadline.clone();
                    let interrupt_stop = stop.clone();
                    runtime.set_interrupt_handler(Some(Box::new(move || {
                        interrupt_stop.load(Ordering::Relaxed)
                            || Instant::now() > *interrupt.lock().unwrap_or_else(|v| v.into_inner())
                    })));
                    let context = Context::full(&runtime)?;
                    let host = Rc::new(RefCell::new(Host::new(options, sender, events)?));
                    context.with(|ctx| -> Result<()> {
                        let callback = host.clone();
                        let function =
                            Function::new(ctx.clone(), move |message: String| -> String {
                                let response = (|| -> Result<Value> {
                                    let data = unpack(&serde_json::from_str(&message)?)?;
                                    callback.borrow_mut().handle(&data)
                                })();
                                match response {
                                    Ok(response) => response.to_string(),
                                    Err(e) => json!({"error":format!("{e:#}")}).to_string(),
                                }
                            })
                            .map_err(|e| js_error(&ctx, e))?;
                        ctx.globals()
                            .set("__nativeHost", function)
                            .map_err(|e| js_error(&ctx, e))?;
                        ctx.globals()
                            .set("appVersion", host.borrow().options.version.clone())
                            .map_err(|e| js_error(&ctx, e))?;
                        ctx.eval::<(), _>(include_str!("script_bridge.js"))
                            .map_err(|e| js_error(&ctx, e))?;
                        ctx.eval::<(), _>(include_str!("../../assets/init.js"))
                            .map_err(|e| js_error(&ctx, e))?;
                        ctx.eval::<(), _>(include_str!("source_bridge.js"))
                            .map_err(|e| js_error(&ctx, e))?;
                        ctx.eval::<(), _>(include_str!("image_bridge.js"))
                            .map_err(|e| js_error(&ctx, e))?;
                        Ok(())
                    })?;
                    let _ = ready.send(Ok(()));
                    while !stop.load(Ordering::Relaxed) {
                        *deadline.lock().unwrap_or_else(|v| v.into_inner()) =
                            Instant::now() + Duration::from_secs(30);
                        match incoming.recv_timeout(Duration::from_millis(10)) {
                            Ok(Command::Eval(code, reply)) => {
                                let result = context.with(|ctx| evaluate(&ctx, &code));
                                let _ = reply.send(result);
                            }
                            Ok(Command::Configure(options, reply)) => {
                                host.borrow_mut().options = *options;
                                let _ = reply.send(Ok("null".into()));
                            }
                            Ok(Command::Settle(id, result)) => {
                                let (value, error) = match result {
                                    Ok(value) => (pack(&value), Value::Null),
                                    Err(e) => (json!(["null"]), json!(format!("{e:#}"))),
                                };
                                context.with(|ctx| {
                                    let _ = ctx.eval::<(), _>(format!(
                                        "__venera.settle({id},{value},{error})"
                                    ));
                                });
                            }
                            Ok(Command::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                        for _ in 0..1024 {
                            match runtime.execute_pending_job() {
                                Ok(true) => {}
                                _ => break,
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = result {
                    let _ = ready.send(Err(e));
                }
                stop.store(true, Ordering::Relaxed);
            })?;
        initialized
            .recv()
            .context("Script worker exited during initialization")??;
        Ok(Self {
            commands,
            closed,
            events: tokio::sync::Mutex::new(event_receiver),
        })
    }
    pub fn query(&self, code: String) -> Result<String> {
        ensure!(
            !self.closed.load(Ordering::Relaxed),
            "JavaScript runtime is closed"
        );
        let (reply, receive) = mpsc::sync_channel(1);
        self.commands
            .send(Command::Eval(code, reply))
            .context("Script worker stopped")?;
        receive.recv().context("Script worker stopped")?
    }
    pub fn configure(&self, options: Options) -> Result<()> {
        let (reply, receive) = mpsc::sync_channel(1);
        self.commands
            .send(Command::Configure(Box::new(options), reply))
            .context("Script worker stopped")?;
        receive.recv().context("Script worker stopped")??;
        Ok(())
    }
    pub fn evaluate(&self, source: &str) -> Result<String> {
        self.query(format!(
            "__venera.evaluate({})",
            serde_json::to_string(source)?
        ))
    }
    pub fn settle(&self, id: u64, value: Value, error: Option<String>) -> Result<()> {
        self.commands
            .send(Command::Settle(
                id,
                error.map_or(Ok(value), |e| Err(anyhow!(e))),
            ))
            .context("Script worker stopped")
    }
    pub async fn promise(&self, id: u64) -> Result<String> {
        struct Pending<'a>(&'a Engine, u64);
        impl Drop for Pending<'_> {
            fn drop(&mut self) {
                let _ = self
                    .0
                    .query(format!("(__venera.discardPromise({}),'null')", self.1));
            }
        }
        let _pending = Pending(self, id);
        loop {
            let result: Value =
                serde_json::from_str(&self.query(format!("__venera.promise({id})"))?)?;
            if let Some(error) = result.get("error") {
                return Err(anyhow!(error
                    .as_str()
                    .unwrap_or("JavaScript rejection")
                    .to_owned()));
            }
            if let Some(value) = result.get("value") {
                return Ok(value.to_string());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        let _ = self.commands.send(Command::Wake);
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn unpack(wire: &Value) -> Result<Value> {
    let kind = wire[0].as_str().context("Invalid bridge value")?;
    Ok(match kind {
        "null" => Value::Null,
        "string" | "bool" | "number" => wire[1].clone(),
        "bigint" => wire[1].clone(),
        "bytes" => json!({"$bytes":wire[1]}),
        "function" => json!({"$function":wire[1]}),
        "array" => Value::Array(
            wire[1]
                .as_array()
                .context("Invalid array")?
                .iter()
                .map(unpack)
                .collect::<Result<_>>()?,
        ),
        "object" => Value::Object(
            wire[1]
                .as_array()
                .context("Invalid object")?
                .iter()
                .map(|entry| {
                    Ok((
                        entry[0].as_str().context("Invalid key")?.to_owned(),
                        unpack(&entry[1])?,
                    ))
                })
                .collect::<Result<_>>()?,
        ),
        _ => return Err(anyhow!("Unsupported bridge value {kind}")),
    })
}
pub fn pack(value: &Value) -> Value {
    match value {
        Value::Null => json!(["null"]),
        Value::Bool(v) => json!(["bool", v]),
        Value::Number(v) => json!(["number", v]),
        Value::String(v) => json!(["string", v]),
        Value::Array(v) => json!(["array", v.iter().map(pack).collect::<Vec<_>>()]),
        Value::Object(v) if v.len() == 1 && v.contains_key("$bytes") => {
            json!(["bytes", v["$bytes"]])
        }
        Value::Object(v) if v.len() == 1 && v.contains_key("$function") => {
            json!(["function", v["$function"]])
        }
        Value::Object(v) => json!([
            "object",
            v.iter()
                .map(|(k, v)| json!([k, pack(v)]))
                .collect::<Vec<_>>()
        ]),
    }
}

struct Host {
    options: Options,
    documents: crate::script_html::Documents,
    commands: mpsc::Sender<Command>,
    events: tokio::sync::mpsc::Sender<String>,
    next: u64,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Host {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl Host {
    fn new(
        options: Options,
        commands: mpsc::Sender<Command>,
        events: tokio::sync::mpsc::Sender<String>,
    ) -> Result<Self> {
        std::fs::create_dir_all(PathBuf::from(&options.data_path).join("comic_source"))?;
        Ok(Self {
            options,
            documents: Default::default(),
            commands,
            events,
            next: 1,
            tasks: vec![],
        })
    }
    fn asynchronous(
        &mut self,
        future: impl std::future::Future<Output = Result<Value>> + Send + 'static,
    ) -> Value {
        let ticket = self.next;
        self.next += 1;
        let commands = self.commands.clone();
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(executor().spawn(async move {
            let result = future.await;
            let _ = commands.send(Command::Settle(ticket, result));
        }));
        json!({"ticket":ticket})
    }
    fn handle(&mut self, data: &Value) -> Result<Value> {
        let method = data["method"]
            .as_str()
            .context("Missing script host method")?;
        let value = match method {
            "modify_image" => {
                let bytes = crate::script_crypto::bytes(&data["data"])?;
                let script = text(data, "script")?.to_owned();
                return Ok(self.asynchronous(async move {
                    Ok(crate::script_crypto::binary(
                        crate::api::media::modify_image(bytes, script).await?,
                    ))
                }));
            }
            "html" => self.documents.handle(data)?,
            "convert" => crate::script_crypto::convert(data)?,
            "random" => {
                use rand::Rng;
                let min = data["min"].as_f64().unwrap_or(0.);
                let max = data["max"].as_f64().unwrap_or(1.);
                let value = min + (max - min) * rand::thread_rng().gen::<f64>();
                if data["type"] == "double" {
                    json!(value)
                } else {
                    json!(value as i64)
                }
            }
            "uuid" => uuid::Uuid::now_v1(&rand::random::<[u8; 6]>())
                .to_string()
                .into(),
            "getLocale" => self.options.locale.clone().into(),
            "getPlatform" => self.options.platform.clone().into(),
            "load_data" => source_data(&self.options.data_path, text(data, "key")?)?
                [text(data, "data_key")?]
            .clone(),
            "isLogged" => (!source_data(&self.options.data_path, text(data, "key")?)?["account"]
                .is_null())
            .into(),
            "save_data" | "delete_data" => {
                let key = text(data, "key")?;
                let field = text(data, "data_key")?;
                ensure!(field != "setting", "setting is not allowed to be saved");
                update_source(
                    &self.options.data_path,
                    key,
                    &[field.into()],
                    if method == "delete_data" {
                        None
                    } else {
                        Some(data["data"].clone())
                    },
                )?;
                let _ = self
                    .events
                    .try_send(json!({"event":"sourceChanged","key":key}).to_string());
                Value::Null
            }
            "cookie" => {
                let mut cookies = crate::cookies::Cookies::open(
                    &PathBuf::from(&self.options.data_path)
                        .join("cookie.db")
                        .to_string_lossy(),
                )?;
                let url = text(data, "url")?;
                match text(data,"function")? {
                    "get"=>Value::Array(cookies.load(url)?.into_iter().map(|c|json!({"name":c.name,"value":c.value,"domain":c.domain,"path":c.path,"expires":c.expires_ms,"max-age":c.max_age,"secure":c.secure,"httpOnly":c.http_only,"session":c.expires_ms.is_none()})).collect()),
                    "set"=>{let values=data["cookies"].as_array().context("Missing cookies")?.iter().map(|c|Ok(crate::api::cookies::StoredCookie{name:text(c,"name")?.into(),value:text(c,"value")?.into(),domain:c["domain"].as_str().map(str::to_owned),path:c["path"].as_str().map(str::to_owned),expires_ms:None,max_age:None,secure:c["secure"]==true,http_only:c["httpOnly"]==true})).collect::<Result<_>>()?;cookies.save(url,values)?;Value::Null},
                    "delete"=>{cookies.delete(url,None)?;Value::Null},_=>return Err(anyhow!("Unknown cookie operation")),
                }
            }
            "delay" => {
                let delay = data["time"].as_u64().unwrap_or(0);
                return Ok(self.asynchronous(async move {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    Ok(Value::Null)
                }));
            }
            "http" => {
                let data = data.clone();
                let options = self.options.clone();
                return Ok(self.asynchronous(async move {
                    Ok(match http(&options, &data).await {
                        Ok(v) => v,
                        Err(e) => {
                            json!({"status":null,"headers":{},"body":null,"error":format!("{e:#}")})
                        }
                    })
                }));
            }
            "compute" => {
                let source = text(data, "function")?.to_owned();
                let args = data.get("args").cloned().unwrap_or_else(|| json!([]));
                let options = self.options.clone();
                return Ok(self.asynchronous(async move { compute(options, source, args).await }));
            }
            "log" => {
                self.events
                    .try_send(json!({"event":"log","data":pack(data)}).to_string())
                    .context("Script event queue full")?;
                Value::Null
            }
            "UI" | "setClipboard" | "getClipboard" => {
                ensure!(
                    self.options.worker_depth == 0,
                    "Platform UI is unavailable inside compute workers"
                );
                let ticket = self.next;
                self.next += 1;
                let immediate = method == "UI"
                    && matches!(
                        data["function"].as_str(),
                        Some("showMessage" | "launchUrl" | "cancelLoading" | "showLoading")
                    );
                self.events.try_send(json!({"event":"platform","ticket":ticket,"data":pack(data),"reply":!immediate}).to_string()).context("Script event queue full")?;
                if !immediate {
                    return Ok(json!({"ticket":ticket}));
                }
                if data["function"] == "showLoading" {
                    json!(ticket)
                } else {
                    Value::Null
                }
            }
            _ => return Err(anyhow!("Unknown script host method: {method}")),
        };
        Ok(json!({"value":pack(&value)}))
    }
}
fn text<'a>(data: &'a Value, key: &str) -> Result<&'a str> {
    data[key].as_str().with_context(|| format!("Missing {key}"))
}

static DATA_WRITES: Mutex<()> = Mutex::new(());
fn source_path(directory: &str, key: &str) -> Result<PathBuf> {
    ensure!(
        !key.is_empty() && key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'),
        "Invalid source key"
    );
    Ok(PathBuf::from(directory)
        .join("comic_source")
        .join(format!("{key}.data")))
}
pub fn source_data(directory: &str, key: &str) -> Result<Value> {
    let path = source_path(directory, key)?;
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e.into()),
    }
}
pub fn update_source(
    directory: &str,
    key: &str,
    path: &[String],
    value: Option<Value>,
) -> Result<()> {
    let _guard = DATA_WRITES
        .lock()
        .map_err(|_| anyhow!("Source data lock poisoned"))?;
    let mut data = source_data(directory, key)?;
    fn apply(current: &mut Value, path: &[String], value: Option<Value>) -> Result<()> {
        if path.is_empty() {
            *current = value.unwrap_or_else(|| json!({}));
            return Ok(());
        }
        if let Some(array) = current.as_array_mut() {
            let index: usize = path[0].parse()?;
            ensure!(index < array.len(), "Source data array index out of range");
            if path.len() == 1 && value.is_none() {
                array.remove(index);
                return Ok(());
            }
            return apply(&mut array[index], &path[1..], value);
        }
        let object = current
            .as_object_mut()
            .context("Expected source data object")?;
        if path.len() == 1 {
            match value {
                Some(value) => {
                    object.insert(path[0].clone(), value);
                }
                None => {
                    object.remove(&path[0]);
                }
            }
        } else {
            apply(
                object.entry(path[0].clone()).or_insert_with(|| json!({})),
                &path[1..],
                value,
            )?;
        }
        Ok(())
    }
    apply(&mut data, path, value)?;
    let target = source_path(directory, key)?;
    std::fs::create_dir_all(target.parent().unwrap())?;
    let mut file = tempfile::NamedTempFile::new_in(target.parent().unwrap())?;
    serde_json::to_writer(file.as_file_mut(), &data)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(target)?;
    Ok(())
}

async fn http(options: &Options, data: &Value) -> Result<Value> {
    let url = text(data, "url")?;
    let mut request =
        crate::api::http::client(&options.http)?.request(text(data, "http_method")?.parse()?, url);
    let headers = data["headers"].as_object();
    let mut has_agent = false;
    let mut has_cookie = false;
    if let Some(headers) = headers {
        for (key, value) in headers {
            if ["cache-time", "prevent-parallel", "http_client"]
                .contains(&key.to_lowercase().as_str())
            {
                continue;
            }
            has_agent |= key.eq_ignore_ascii_case("user-agent");
            has_cookie |= key.eq_ignore_ascii_case("cookie");
            request = request.header(
                key,
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string()),
            );
        }
    }
    if !has_agent {
        request = request.header("User-Agent", &options.user_agent);
    }
    let cookie_path = PathBuf::from(&options.data_path)
        .join("cookie.db")
        .to_string_lossy()
        .into_owned();
    if !has_cookie {
        let cookie = crate::cookies::Cookies::open(&cookie_path)?
            .load(url)?
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ");
        if cookie.contains("cf_clearance=") && !options.cloudflare_user_agent.is_empty() {
            request = request.header("User-Agent", &options.cloudflare_user_agent);
        }
        if !cookie.is_empty() {
            request = request.header("Cookie", cookie);
        }
    }
    let body = &data["data"];
    if let Some(text) = body.as_str() {
        request = request.body(text.to_owned());
    } else if body.get("$bytes").is_some() || body.is_array() {
        request = request.body(crate::script_crypto::bytes(body)?);
    } else if body.is_object() {
        let form = headers
            .and_then(|v| {
                v.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            })
            .and_then(|(_, v)| v.as_str())
            .is_some_and(|v| v.contains("application/x-www-form-urlencoded"));
        if form {
            request = request.form(body.as_object().unwrap());
        } else {
            request = request
                .header("Content-Type", "application/json")
                .body(body.to_string());
        }
    }
    let request = request.build()?;
    let mut request_headers: Vec<_> = request
        .headers()
        .iter()
        .map(|(k, v)| crate::api::http::HttpHeader {
            name: k.to_string(),
            value: String::from_utf8_lossy(v.as_bytes()).into_owned(),
        })
        .collect();
    if let Some(headers) = data["headers"].as_object() {
        for (key, value) in headers {
            if crate::http_policy::is_policy(key) {
                request_headers.push(crate::api::http::HttpHeader {
                    name: key.clone(),
                    value: value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                });
            }
        }
    }
    let image_request = data["_requestId"]
        .as_str()
        .and_then(crate::image_tasks::request);
    let cancellation = image_request
        .as_ref()
        .map(|r| r.cancellation.clone())
        .unwrap_or_else(|| Arc::new(crate::api::http::HttpCancellation::new()));
    let response = crate::api::http::http_request(
        request.url().to_string(),
        request.method().to_string(),
        request_headers,
        request.body().and_then(|v| v.as_bytes().map(Vec::from)),
        &crate::api::http::HttpUpload::new(),
        false,
        options.http.clone(),
        &cancellation,
    )
    .await?;
    let status = response.status();
    let mut headers = HashMap::<String, String>::new();
    let mut cookies = Vec::new();
    for header in response.headers() {
        if header.name.eq_ignore_ascii_case("set-cookie") {
            cookies.push(header.value.clone());
        }
        headers
            .entry(header.name)
            .and_modify(|v| {
                v.push(',');
                v.push_str(&header.value);
            })
            .or_insert(header.value);
    }
    if status == 403
        && headers
            .get("cf-mitigated")
            .is_some_and(|v| v == "challenge")
    {
        response.close().await;
        return Err(anyhow!("CloudflareException: {url}"));
    }
    crate::cookies::Cookies::open(&cookie_path)?.save_headers(&response.url(), cookies)?;
    let mut bytes = Vec::new();
    let total = headers
        .get("content-length")
        .and_then(|v| v.parse::<u64>().ok());
    while let Some(chunk) = response.next_chunk(&cancellation).await? {
        bytes.extend(chunk);
        if let Some(progress) = &image_request {
            progress.progress(bytes.len() as u64, total);
        }
    }
    let body = if data["bytes"] == true {
        crate::script_crypto::binary(bytes)
    } else {
        String::from_utf8_lossy(&bytes).into_owned().into()
    };
    Ok(json!({"status":status,"headers":headers,"body":body,"error":null}))
}

pub async fn compute(mut options: Options, source: String, args: Value) -> Result<Value> {
    static LIMIT: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    ensure!(
        options.worker_depth < 8,
        "Maximum nested compute depth exceeded"
    );
    let _permit = if options.worker_depth == 0 {
        Some(
            LIMIT
                .get_or_init(|| tokio::sync::Semaphore::new(4))
                .acquire()
                .await?,
        )
    } else {
        None
    };
    options.worker_depth += 1;
    let engine = Engine::open(options)?;
    let result: Value = serde_json::from_str(
        &engine.evaluate(&format!("({source})(...__venera.unpack({}))", pack(&args)))?,
    )?;
    if result[0] == "promise" {
        let result = engine
            .promise(result[1].as_u64().context("Invalid promise")?)
            .await?;
        unpack(&serde_json::from_str(&result)?)
    } else {
        unpack(&result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options(path: &str) -> Options {
        Options {
            worker_depth: 0,
            data_path: path.into(),
            version: "1.6.3".into(),
            locale: "zh_CN".into(),
            platform: "test".into(),
            user_agent: "Venera test".into(),
            cloudflare_user_agent: String::new(),
            http: crate::api::http::HttpOptions {
                proxy: None,
                dns_overrides: HashMap::new(),
                tls_sni: true,
                verify_certificates: true,
                connect_timeout_ms: 1000,
                receive_timeout_ms: 1000,
                max_redirects: 5,
            },
        }
    }
    #[test]
    fn runtime_binary_html_functions_and_source_storage() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let engine = Engine::open(options(temp.path().to_str().unwrap()))?;
        let eval =
            |s: &str| -> Result<Value> { unpack(&serde_json::from_str(&engine.evaluate(s)?)?) };
        assert_eq!(
            eval("Convert.decodeUtf8(Convert.encodeUtf8('漫画'))")?,
            "漫画"
        );
        assert_eq!(
            eval("Convert.encodeBase64(new Uint8Array([0,255]).buffer)")?,
            "AP8="
        );
        assert_eq!(
            eval("new HtmlDocument('<p>A<b>B</b></p>').querySelector('p').text")?,
            "AB"
        );
        let function: Value = serde_json::from_str(&engine.evaluate("x=>x+1")?)?;
        let id = function[1].as_u64().unwrap();
        assert_eq!(
            unpack(&serde_json::from_str(&engine.query(format!(
                "__venera.call({id},['array',[['number',4]]])"
            ))?)?)?,
            5
        );
        eval("sendMessage({method:'save_data',key:'test',data_key:'account',data:['a','b']})")?;
        assert_eq!(eval("sendMessage({method:'isLogged',key:'test'})")?, true);
        assert_eq!(
            source_data(temp.path().to_str().unwrap(), "test")?["account"],
            json!(["a", "b"])
        );
        assert!(eval("throw new Error('expected error')")
            .unwrap_err()
            .to_string()
            .contains("expected error"));
        engine.close();
        assert!(engine.evaluate("1").is_err());
        Ok(())
    }
    #[tokio::test]
    async fn runtime_promises_ui_events_and_compute() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let options = options(temp.path().to_str().unwrap());
        let engine = Engine::open(options.clone())?;
        let promise: Value = serde_json::from_str(
            &engine.evaluate("new Promise(resolve=>setTimeout(()=>resolve(42),5))")?,
        )?;
        assert_eq!(
            unpack(&serde_json::from_str(
                &engine.promise(promise[1].as_u64().unwrap()).await?
            )?)?,
            42
        );
        let promise: Value = serde_json::from_str(&engine.evaluate("getClipboard()")?)?;
        let event: Value = serde_json::from_str(&engine.events.lock().await.recv().await.unwrap())?;
        engine.settle(event["ticket"].as_u64().unwrap(), json!("clipboard"), None)?;
        assert_eq!(
            unpack(&serde_json::from_str(
                &engine.promise(promise[1].as_u64().unwrap()).await?
            )?)?,
            "clipboard"
        );
        assert_eq!(
            compute(options, "async (a,b)=>a+b".into(), json!([3, 4])).await?,
            7
        );
        Ok(())
    }
}
