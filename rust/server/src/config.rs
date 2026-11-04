use crate::{Error, Result, store::text};
use fs2::FileExt;
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

pub struct Settings {
    pub user_root: PathBuf,
    pub(crate) codex: crate::codex_session::Runtime,
    mutations: std::sync::Mutex<()>,
    _lock: HomeLock,
}
pub(crate) struct HomeLock(std::fs::File);
impl Drop for HomeLock {
    fn drop(&mut self) {
        // Explicitly release flock before closing. A concurrently spawning
        // child can briefly inherit the open file description until exec;
        // closing only our descriptor can otherwise delay ownership release.
        let _ = FileExt::unlock(&self.0);
    }
}
pub struct Model {
    pub model: String,
    pub profile_id: String,
    pub model_id: String,
    pub api_key: String,
    pub base_url: String,
    pub format: String,
    pub binding: String,
    pub effort: Option<String>,
    pub extra_headers: Value,
    pub account_binding: Option<String>,
    pub context_window: u64,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
}
pub fn read_json(path: &Path, default: Value) -> Result<Value> {
    match fs::read(path) {
        Ok(data) => serde_json::from_slice(&data)
            .map_err(|_| Error::conflict("Runtime settings contain invalid JSON.")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default),
        Err(e) => Err(e.into()),
    }
}
pub fn write_json(path: &Path, value: &Value) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::invalid("Invalid settings location."))?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(value.to_string().as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|_| Error::conflict("Unable to atomically save settings."))?;
    Ok(())
}
pub fn safe_path(root: &Path, path: &Path) -> Result<()> {
    if !path.starts_with(root) {
        return Err(Error::invalid("Path escapes runtime home."));
    }
    let mut current = path.to_path_buf();
    loop {
        if let Ok(meta) = fs::symlink_metadata(&current)
            && meta.file_type().is_symlink()
        {
            return Err(Error::conflict(
                "Runtime storage cannot use symbolic links.",
            ));
        }
        if current == root {
            break;
        }
        if !current.pop() {
            return Err(Error::invalid("Invalid runtime path."));
        }
    }
    Ok(())
}
impl Settings {
    pub(crate) fn lock_home(home: &Path) -> Result<(PathBuf, HomeLock)> {
        fs::create_dir_all(home)?;
        let home = home.canonicalize()?;
        if home.join(crate::recovery::INCOMPLETE).try_exists()? {
            return Err(Error::conflict(
                "Recovery publication is incomplete. Restore the backup to a new home.",
            ));
        }
        let user_root = home.join("data/user");
        safe_path(&home, &user_root.join("settings"))?;
        fs::create_dir_all(&user_root)?;
        let lock_path = user_root.join("rust-server.lock");
        safe_path(&home, &lock_path)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        lock.try_lock_exclusive()
            .map_err(|_| Error::conflict("A Rust server already owns this home."))?;
        Ok((user_root, HomeLock(lock)))
    }
    pub fn open(home: &Path) -> Result<Self> {
        let (user_root, lock) = Self::lock_home(home)?;
        fs::create_dir_all(user_root.join("settings"))?;
        for filename in ["auth.json", "integrations.json"] {
            safe_path(&user_root, &user_root.join("settings").join(filename))?;
        }
        let auth = read_json(&user_root.join("settings/auth.json"), json!({}))?;
        let env_auth = std::env::var("DEEPTUTOR_AUTH_ENABLED").unwrap_or_default();
        if auth["enabled"] == true
            || matches!(env_auth.to_lowercase().as_str(), "1" | "true" | "yes")
        {
            return Err(Error::unsupported(
                "This native server currently supports local single-user mode. Existing authentication must not be bypassed.",
            ));
        }
        let integrations = read_json(&user_root.join("settings/integrations.json"), json!({}))?;
        if !text(&integrations, "pocketbase_url").is_empty()
            || std::env::var("POCKETBASE_URL").is_ok_and(|s| !s.is_empty())
        {
            return Err(Error::unsupported(
                "PocketBase storage is configured and has not yet been migrated to this native server.",
            ));
        }
        let settings = Self {
            user_root,
            codex: Default::default(),
            mutations: std::sync::Mutex::new(()),
            _lock: lock,
        };
        settings.recover_apply()?;
        Ok(settings)
    }
    pub fn catalog(&self) -> Result<Value> {
        let _guard = self.settings_lock()?;
        self.catalog_locked()
    }
    pub(crate) fn catalog_locked(&self) -> Result<Value> {
        let path = self.user_root.join("settings/model_catalog.json");
        safe_path(&self.user_root, &path)?;
        let mut value = read_json(&path, json!({"version":1,"connections":[],"services":{}}))?;
        if !value.is_object() || !value["services"].is_object() {
            return Err(Error::conflict("Invalid model catalog."));
        }
        if value["connections"].is_null() {
            value["connections"] = json!([]);
        }
        for service in [
            "llm",
            "task",
            "embedding",
            "search",
            "tts",
            "stt",
            "imagegen",
            "videogen",
        ] {
            if value["services"][service].is_null() {
                value["services"][service] =
                    json!({"active_profile_id":null,"active_model_id":null,"profiles":[]});
            }
        }
        validate_catalog(&value)?;
        Ok(value)
    }
    pub fn save_catalog(&self, proposed: Value) -> Result<Value> {
        let _guard = self.settings_lock()?;
        self.save_catalog_locked(proposed)
    }
    pub(crate) fn save_catalog_locked(&self, proposed: Value) -> Result<Value> {
        let restored = self.prepare_catalog(proposed)?;
        write_json(
            &self.user_root.join("settings/model_catalog.json"),
            &restored,
        )?;
        Ok(redact(restored))
    }
    pub(crate) fn prepare_catalog(&self, proposed: Value) -> Result<Value> {
        if !proposed.is_object()
            || !proposed["services"].is_object()
            || !proposed["services"]["llm"]["profiles"].is_array()
        {
            return Err(Error::invalid("Invalid model catalog."));
        }
        restore_catalog(proposed, &self.catalog_locked()?)
    }
    pub(crate) fn settings_lock(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        let guard = self
            .mutations
            .lock()
            .map_err(|_| Error::conflict("Settings lock failed."))?;
        self.recover_apply()?;
        Ok(guard)
    }
    fn apply_path(&self, name: &str) -> Result<PathBuf> {
        let path = self.user_root.join("settings").join(name);
        safe_path(&self.user_root, &path)?;
        Ok(path)
    }
    fn recover_apply(&self) -> Result<()> {
        let path = self.apply_path("pending_apply.json")?;
        if !path.try_exists()? {
            return Ok(());
        }
        let pending = read_json(&path, Value::Null)?;
        if pending["version"] != 1
            || !pending["catalog"]["services"].is_object()
            || !pending["draft"].is_object()
            || (!pending["draft"]["catalog"].is_null() && !pending["draft"]["catalog"].is_object())
            || (!pending["draft"]["extensions"].is_null()
                && !pending["draft"]["extensions"].is_object())
        {
            return Err(Error::conflict("Invalid pending settings apply."));
        }
        validate_catalog(&pending["catalog"])?;
        if pending["draft"]["catalog"].is_object() {
            validate_catalog(&pending["draft"]["catalog"])?;
        }
        write_json(&self.apply_path("model_catalog.json")?, &pending["catalog"])?;
        write_json(&self.apply_path("settings_draft.json")?, &pending["draft"])?;
        fs::remove_file(path)?;
        Ok(())
    }
    pub(crate) fn publish_apply_locked(&self, catalog: &Value, draft: &Value) -> Result<()> {
        write_json(
            &self.apply_path("pending_apply.json")?,
            &json!({"version":1,"catalog":catalog,"draft":draft}),
        )?;
        self.recover_apply()
    }
    pub fn model(&self, selection: &Value) -> Result<Model> {
        resolve_model(&self.catalog()?, selection)
    }
    pub fn options(&self) -> Result<Value> {
        let catalog = self.catalog()?;
        let llm = &catalog["services"]["llm"];
        let mut options = Vec::new();
        for p in llm["profiles"].as_array().into_iter().flatten() {
            for m in p["models"].as_array().into_iter().flatten() {
                let selection = json!({"profile_id":p["id"],"model_id":m["id"]});
                let Ok(model) = resolve_model(&catalog, &selection) else {
                    continue;
                };
                options.push(json!({"profile_id":p["id"],"model_id":m["id"],"profile_name":p["name"],
                 "model_name":m.get("name").unwrap_or(&m["model"]),"model":model.model,
                 "provider":model.binding,"provider_label":model.binding,
                 "is_active_default":p["id"]==llm["active_profile_id"]&&m["id"]==llm["active_model_id"],
                 "supported_reasoning_efforts":efforts(m,&model.binding),"reasoning_effort":model.effort,"supports_vision":false}));
            }
        }
        Ok(
            json!({"active":if llm["active_profile_id"].is_string()&&llm["active_model_id"].is_string(){json!({"profile_id":llm["active_profile_id"],"model_id":llm["active_model_id"]})}else{Value::Null},"options":options}),
        )
    }
}
fn efforts(model: &Value, binding: &str) -> Value {
    for key in [
        "codex_supported_reasoning_levels",
        "supported_reasoning_efforts",
        "supported_reasoning_levels",
    ] {
        if model[key].is_array() {
            return model[key].clone();
        }
    }
    if model["capabilities"]["reasoning"] == false {
        return json!([]);
    }
    let name = text(model, "model").to_lowercase();
    if binding == "deepseek" || (binding == "custom" && name.contains("deepseek")) {
        return json!(["minimal", "high"]);
    }
    if binding == "dashscope" || (binding == "custom" && name.contains("qwen")) {
        return json!(["minimal", "high"]);
    }
    if matches!(binding, "openai" | "openai_codex") {
        if name.contains("gpt-5.6-sol") {
            return json!(["none", "low", "medium", "high", "xhigh", "max"]);
        }
        if name.contains("gpt-5") || name.contains("codex") {
            return json!(["minimal", "low", "medium", "high", "xhigh"]);
        }
        if ["o1", "o3", "o4"].iter().any(|p| name.starts_with(p)) {
            return json!(["low", "medium", "high"]);
        }
    }
    if model["capabilities"]["reasoning"] == true {
        return json!(["none", "low", "medium", "high"]);
    }
    json!([])
}
fn effective_profile(catalog: &Value, profile: &Value, model: &Value) -> Result<Value> {
    let reference = model
        .get("provider_ref")
        .filter(|r| !r.is_null())
        .or_else(|| profile.get("provider_ref").filter(|r| !r.is_null()));
    if reference.is_some_and(|r| !r.is_object()) {
        return Err(Error::invalid("Invalid provider reference."));
    }
    let legacy = profile["connection_id"].as_str().filter(|s| !s.is_empty());
    let source = if let Some(reference) = reference.filter(|r| r.is_object()) {
        if let Some(id) = reference["connection_id"].as_str() {
            catalog["connections"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| c["id"] == id)
        } else {
            catalog["services"][text(reference, "service")]["profiles"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["id"] == reference["profile_id"] && p["provider_ref"].is_null())
        }
        .ok_or_else(|| Error::invalid("Referenced provider is no longer configured."))?
    } else if let Some(id) = legacy {
        catalog["connections"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|c| c["id"] == id)
            .ok_or_else(|| Error::invalid("Provider connection is missing."))?
    } else {
        profile
    };
    let mut result = profile.clone();
    if !std::ptr::eq(source, profile) {
        for key in [
            "api_key",
            "api_version",
            "extra_headers",
            "proxy",
            "app_id",
            "base_url",
            "binding",
            "api_format",
            "wire_api",
            "codex_account_binding",
        ] {
            result[key] = source.get(key).cloned().unwrap_or(Value::Null);
        }
        if text(&result, "binding").is_empty() {
            result["binding"] = source["provider"].clone();
        }
        if text(&result, "binding").is_empty()
            && let Some(reference) = reference
        {
            result["binding"] = reference["binding"].clone();
        }
        if text(&result, "base_url").is_empty()
            && let Some(reference) = reference
        {
            result["base_url"] = reference["default_base_url"].clone();
        }
    }
    let overrides = &source["service_overrides"]["llm"];
    if overrides["enabled"] == true {
        for key in ["binding", "base_url"] {
            if !text(overrides, key).is_empty() {
                result[key] = overrides[key].clone();
            }
        }
    }
    Ok(result)
}
pub fn resolve_model(catalog: &Value, selection: &Value) -> Result<Model> {
    let llm = &catalog["services"]["llm"];
    let pid = selection["profile_id"]
        .as_str()
        .unwrap_or(text(llm, "active_profile_id"));
    let mid = selection["model_id"]
        .as_str()
        .unwrap_or(text(llm, "active_model_id"));
    let profile = llm["profiles"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["id"] == pid)
        .ok_or_else(|| Error::invalid("Select a configured model in Settings."))?;
    let model = profile["models"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|m| m["id"] == mid)
        .ok_or_else(|| Error::invalid("Selected model is not configured."))?;
    let effective = effective_profile(catalog, profile, model)?;
    let field = |key: &str| effective.get(key).cloned().unwrap_or(Value::Null);
    if field("api_key").is_array() {
        return Err(Error::unsupported(
            "API key pools have not yet been migrated; configure a single key for this native profile.",
        ));
    }
    let binding = field("binding")
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("custom")
        .replace('-', "_");
    let raw_format = field("api_format");
    let wire = field("wire_api");
    let format = match raw_format.as_str().unwrap_or("auto") {
        "auto" => wire.as_str().filter(|s| *s != "auto").unwrap_or(
            if binding == "anthropic" || binding == "custom_anthropic" {
                "anthropic"
            } else {
                "chat_completions"
            },
        ),
        other => other,
    }
    .to_string();
    if !matches!(
        format.as_str(),
        "openai_chat"
            | "chat_completions"
            | "openai"
            | "responses"
            | "openai_responses"
            | "anthropic"
    ) && binding != "openai_codex"
    {
        return Err(Error::unsupported(
            "This provider protocol has not yet been migrated to the native server.",
        ));
    }
    if !text(&effective, "proxy").is_empty() {
        return Err(Error::unsupported(
            "Per-provider proxies are not yet supported by the native server.",
        ));
    }
    let override_effort = selection["reasoning_effort"]
        .as_str()
        .filter(|s| !s.is_empty());
    let levels = efforts(model, &binding);
    if let Some(effort) = override_effort
        && (![
            "none", "minimal", "low", "medium", "high", "xhigh", "max", "adaptive",
        ]
        .contains(&effort)
            || levels
                .as_array()
                .is_none_or(|v| !v.iter().any(|l| l == effort)))
    {
        return Err(Error::invalid(
            "Reasoning effort is not supported by this model.",
        ));
    }
    let effort = override_effort
        .or_else(|| model["reasoning_effort"].as_str())
        .or_else(|| profile["reasoning_effort"].as_str())
        .map(str::to_owned);
    let base = field("base_url");
    let default = match binding.as_str() {
        "openai" => "https://api.openai.com/v1",
        "openai_codex" => "https://chatgpt.com/backend-api",
        "deepseek" => "https://api.deepseek.com/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        "ollama" => "http://localhost:11434/v1",
        "anthropic" => "https://api.anthropic.com/v1",
        _ => "",
    };
    let base_url = base
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(default)
        .trim_end_matches('/')
        .to_string();
    let url =
        reqwest::Url::parse(&base_url).map_err(|_| Error::invalid("Provider URL is invalid."))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::invalid(
            "Use an HTTP(S) provider base URL without credentials or query parameters.",
        ));
    }
    if text(model, "model").is_empty() {
        return Err(Error::invalid("Model identifier is empty."));
    }
    Ok(Model {
        model: text(model, "model").to_string(),
        profile_id: pid.into(),
        model_id: mid.into(),
        api_key: field("api_key").as_str().unwrap_or("").to_string(),
        base_url,
        format,
        binding,
        effort,
        extra_headers: normalize_headers(field("extra_headers"))?,
        account_binding: profile["codex_account_binding"].as_str().map(str::to_owned),
        context_window: [&model["context_window"], &model["context_window_tokens"]]
            .into_iter()
            .find_map(|v| v.as_u64().or_else(|| v.as_str()?.trim().parse().ok()))
            .filter(|n| *n > 0)
            .unwrap_or(16384),
        max_tokens: model["max_tokens"]
            .as_u64()
            .or_else(|| profile["max_tokens"].as_u64()),
        temperature: model["temperature"]
            .as_f64()
            .or_else(|| profile["temperature"].as_f64()),
    })
}
pub fn normalize_headers(value: Value) -> Result<Value> {
    let value = match value {
        Value::Null => json!({}),
        Value::String(s) if s.trim().is_empty() => json!({}),
        Value::String(s) => serde_json::from_str(&s)
            .map_err(|_| Error::invalid("Extra headers must be a JSON object of strings."))?,
        other => other,
    };
    let headers = value
        .as_object()
        .ok_or_else(|| Error::invalid("Extra headers must be a JSON object of strings."))?;
    for (key, value) in headers {
        reqwest::header::HeaderName::from_bytes(key.as_bytes())
            .map_err(|_| Error::invalid("Invalid extra header name."))?;
        let value = value
            .as_str()
            .ok_or_else(|| Error::invalid("Header values must be strings."))?;
        reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| Error::invalid("Invalid extra header value."))?;
    }
    Ok(value)
}
fn secret(key: &str) -> bool {
    let k = key.to_lowercase();
    k == "extra_headers"
        || ["api_key", "apikey", "token", "secret", "password"]
            .iter()
            .any(|s| k.contains(s))
}
fn mask(value: &mut Value) {
    match value {
        Value::String(s) if !s.is_empty() => *s = "***".into(),
        Value::Array(a) => a.iter_mut().for_each(mask),
        Value::Object(o) => o.values_mut().for_each(mask),
        _ => {}
    }
}
pub fn redact(mut value: Value) -> Value {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(o) => {
                for (k, v) in o {
                    if secret(k) { mask(v) } else { walk(v) }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut value);
    value
}
fn restore(proposed: &mut Value, current: &Value) {
    match proposed {
        Value::String(s) if s == "***" => *proposed = current.clone(),
        Value::Object(map) => {
            for (k, v) in map {
                restore(v, &current[k])
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter_mut().enumerate() {
                restore(v, &current[i])
            }
        }
        _ => {}
    }
}
fn restore_profiles(proposed: &mut Value, current: &Value) {
    for item in proposed.as_array_mut().into_iter().flatten() {
        if let Some(old) = current
            .as_array()
            .into_iter()
            .flatten()
            .find(|c| c["id"] == item["id"])
            && let Some(map) = item.as_object_mut()
        {
            for (k, v) in map {
                if secret(k) {
                    restore(v, &old[k]);
                }
            }
        }
    }
}
pub fn http_client() -> Result<reqwest::Client> {
    // Client clones share the connection pool. Credentials stay on each request.
    static CLIENT: std::sync::OnceLock<std::result::Result<reqwest::Client, reqwest::Error>> =
        std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(20))
                .read_timeout(Duration::from_secs(90))
                .build()
        })
        .as_ref()
        .cloned()
        .map_err(|_| Error::provider("Unable to initialize HTTP client."))
}

pub async fn response_json(response: reqwest::Response, limit: usize) -> Result<Value> {
    use futures_util::StreamExt;
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(Error::provider("Provider response exceeds size limit."));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| Error::provider("Provider response interrupted."))?;
        if body.len() + chunk.len() > limit {
            return Err(Error::provider("Provider response exceeds size limit."));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| Error::provider("Provider returned invalid JSON."))
}

fn identified_items<'a>(value: &'a Value, name: &str) -> Result<&'a Vec<Value>> {
    let items = value
        .as_array()
        .ok_or_else(|| Error::invalid(format!("{name} must be an array.")))?;
    let mut ids = std::collections::HashSet::new();
    for item in items {
        let id = text(item, "id");
        if !item.is_object() || id.trim().is_empty() || id.len() > 200 || !ids.insert(id) {
            return Err(Error::invalid(format!(
                "{name} require unique nonempty IDs of at most 200 bytes."
            )));
        }
    }
    Ok(items)
}
fn validate_catalog(catalog: &Value) -> Result<()> {
    let services = catalog["services"]
        .as_object()
        .ok_or_else(|| Error::invalid("Expected catalog services."))?;
    if !catalog["connections"].is_null() {
        identified_items(&catalog["connections"], "Connections")?;
    }
    for service in services.values() {
        if !service.is_object() {
            return Err(Error::invalid("Expected service object."));
        }
        for profile in identified_items(&service["profiles"], "Profiles")? {
            if !profile["models"].is_null() {
                identified_items(&profile["models"], "Models")?;
            }
        }
    }
    Ok(())
}
pub fn restore_catalog(mut proposed: Value, current: &Value) -> Result<Value> {
    if !proposed.is_object() || !proposed["services"].is_object() {
        return Err(Error::invalid("Expected a catalog with service objects."));
    }
    validate_catalog(&proposed)?;
    restore_profiles(&mut proposed["connections"], &current["connections"]);
    if let Some(services) = proposed["services"].as_object_mut() {
        for (name, service) in services {
            restore_profiles(
                &mut service["profiles"],
                &current["services"][name]["profiles"],
            );
        }
    }
    Ok(proposed)
}
