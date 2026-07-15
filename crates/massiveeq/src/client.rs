use anyhow::{Context, Result};
use massiveeq_core::{ComparisonSet, DeviceInfo, ProfileAnalysis, ProfileInfo};
use std::collections::HashMap;
use zbus::blocking::{Connection, Proxy};

pub struct Client {
    connection: Connection,
}

impl Client {
    pub fn connect() -> Result<Self> {
        let connection = Connection::session().context("could not connect to the user D-Bus")?;
        let client = Self { connection };
        if client.proxy()?.call::<_, _, String>("Ping", &()).is_err() {
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "start", "massiveeq.service"])
                .status();
            std::thread::sleep(std::time::Duration::from_millis(250));
            client
                .proxy()?
                .call::<_, _, String>("Ping", &())
                .context("MassiveEQ service is unavailable")?;
        }
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "enable", "massiveeq.service"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        Ok(client)
    }

    fn proxy(&self) -> Result<Proxy<'_>> {
        Proxy::new(
            &self.connection,
            "org.massiveeq.Service1",
            "/org/massiveeq/Service1",
            "org.massiveeq.Service1",
        )
        .context("could not create service proxy")
    }

    pub fn profiles(&self) -> Result<Vec<ProfileInfo>> {
        self.call_json("ListProfiles", &())
    }
    pub fn devices(&self) -> Result<Vec<DeviceInfo>> {
        self.call_json("ListDevices", &())
    }
    pub fn comparisons(&self) -> Result<HashMap<String, ComparisonSet>> {
        self.call_json("ListComparisons", &())
    }
    pub fn configure_comparison(
        &self,
        device_key: &str,
        profile_ids: &[String],
    ) -> Result<ComparisonSet> {
        let json =
            serde_json::to_string(profile_ids).context("could not encode comparison bank")?;
        self.call_json("ConfigureComparison", &(device_key, json))
    }
    pub fn select_comparison_profile(&self, device_key: &str, profile_id: &str) -> Result<()> {
        self.proxy()?
            .call("SelectComparisonProfile", &(device_key, profile_id))
            .context("comparison switch failed")
    }
    pub fn delete_comparison(&self, device_key: &str) -> Result<()> {
        self.proxy()?
            .call("DeleteComparison", &(device_key))
            .context("comparison bank removal failed")
    }
    pub fn create(&self, name: &str) -> Result<ProfileInfo> {
        self.call_json("CreateProfile", &(name))
    }
    pub fn import(&self, path: &str) -> Result<ProfileInfo> {
        self.call_json("ImportProfile", &(path))
    }
    pub fn set_convolution(&self, id: &str, path: &str, channel: &str) -> Result<ProfileInfo> {
        self.call_json("SetConvolution", &(id, path, channel))
    }
    pub fn put(&self, id: &str, name: &str, text: &str, trim: f64) -> Result<ProfileInfo> {
        self.call_json("PutProfile", &(id, name, text, trim))
    }
    pub fn delete(&self, id: &str) -> Result<()> {
        self.proxy()?
            .call("DeleteProfile", &(id))
            .context("delete failed")
    }
    pub fn export(&self, id: &str, destination: &str) -> Result<()> {
        self.proxy()?
            .call("ExportProfile", &(id, destination))
            .context("export failed")
    }
    pub fn assign(&self, device_key: &str, profile_id: &str) -> Result<()> {
        self.proxy()?
            .call("AssignProfile", &(device_key, profile_id))
            .context("assignment failed")
    }
    pub fn set_device_bypass(&self, device_key: &str, bypassed: bool) -> Result<()> {
        self.proxy()?
            .call("SetDeviceBypass", &(device_key, bypassed))
            .context("bypass failed")
    }
    pub fn set_global_bypass(&self, bypassed: bool) -> Result<()> {
        self.proxy()?
            .call("SetGlobalBypass", &(bypassed))
            .context("global bypass failed")
    }
    pub fn analyze(&self, id: &str) -> Result<ProfileAnalysis> {
        let sample_rate = self
            .status()
            .ok()
            .and_then(|status| {
                status
                    .pointer("/engine/active/0/sample_rate")
                    .and_then(serde_json::Value::as_u64)
            })
            .and_then(|rate| u32::try_from(rate).ok())
            .unwrap_or(48_000);
        self.call_json("Analyze", &(id, sample_rate))
    }
    pub fn status(&self) -> Result<serde_json::Value> {
        self.call_json("Status", &())
    }

    fn call_json<
        R: serde::de::DeserializeOwned,
        B: serde::ser::Serialize + zbus::zvariant::DynamicType,
    >(
        &self,
        method: &str,
        body: &B,
    ) -> Result<R> {
        let json: String = self
            .proxy()?
            .call(method, body)
            .with_context(|| format!("{method} failed"))?;
        serde_json::from_str(&json).with_context(|| format!("invalid {method} response"))
    }
}
