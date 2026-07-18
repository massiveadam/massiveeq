use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use massiveeq_core::{ComparisonSet, DeviceInfo, ProfileInfo, parse_text, serialize_profile};
use massiveeqctl::{Command, QuickSnapshot};
use std::{collections::HashMap, io::Write, time::Duration};
use tokio::time::sleep;
use zbus::{Connection, Proxy};

const DESTINATION: &str = "org.massiveeq.Service1";
const OBJECT_PATH: &str = "/org/massiveeq/Service1";
const INTERFACE: &str = "org.massiveeq.Service1";

struct Client {
    connection: Connection,
}

impl Client {
    async fn connect() -> Result<Self> {
        let connection = Connection::session()
            .await
            .context("could not connect to the user session")?;
        let client = Self { connection };
        client
            .proxy()
            .await?
            .call::<_, _, String>("Ping", &())
            .await
            .context("MassiveEQ audio service is unavailable")?;
        Ok(client)
    }

    async fn proxy(&self) -> Result<Proxy<'_>> {
        Proxy::new(&self.connection, DESTINATION, OBJECT_PATH, INTERFACE)
            .await
            .context("could not connect to MassiveEQ")
    }

    async fn snapshot(&self) -> Result<QuickSnapshot> {
        let profiles: Vec<ProfileInfo> = self.call_json("ListProfiles", &()).await?;
        let devices: Vec<DeviceInfo> = self.call_json("ListDevices", &()).await?;
        let comparisons: HashMap<String, ComparisonSet> =
            self.call_json("ListComparisons", &()).await?;
        let status: serde_json::Value = self.call_json("Status", &()).await?;
        Ok(QuickSnapshot::online(
            profiles,
            devices,
            comparisons,
            &status,
        ))
    }

    async fn set_global_bypass(&self, bypassed: bool) -> Result<()> {
        self.proxy()
            .await?
            .call("SetGlobalBypass", &(bypassed))
            .await
            .context("could not update the engine")
    }

    async fn set_device_bypass(&self, device_key: &str, bypassed: bool) -> Result<()> {
        self.proxy()
            .await?
            .call("SetDeviceBypass", &(device_key, bypassed))
            .await
            .context("could not update output filters")
    }

    async fn assign(&self, device_key: &str, profile_id: &str) -> Result<()> {
        self.proxy()
            .await?
            .call("AssignProfile", &(device_key, profile_id))
            .await
            .context("could not update output assignment")
    }

    async fn compare(&self, device_key: &str, profile_id: &str) -> Result<()> {
        self.proxy()
            .await?
            .call("SelectComparisonProfile", &(device_key, profile_id))
            .await
            .context("could not switch comparison profile")
    }

    async fn set_filter(
        &self,
        profile_id: &str,
        filter_index: usize,
        frequency_hz: f64,
        gain_db: f64,
        q: f64,
    ) -> Result<()> {
        let profile_info: ProfileInfo = self.call_json("GetProfile", &(profile_id)).await?;
        let mut document = parse_text(&profile_info.name, &profile_info.text);
        let filter = document
            .filters
            .get_mut(filter_index)
            .ok_or_else(|| anyhow!("filter {filter_index} was not found"))?;
        filter.frequency = frequency_hz;
        filter.gain_db = gain_db;
        filter.q = q;
        let text = serialize_profile(&document);
        let _: ProfileInfo = self
            .call_json(
                "PutProfile",
                &(
                    profile_id,
                    document.name.as_str(),
                    text.as_str(),
                    profile_info.manual_trim_db,
                ),
            )
            .await
            .context("could not update profile filter")?;
        Ok(())
    }

    async fn call_json<
        R: serde::de::DeserializeOwned,
        B: serde::ser::Serialize + zbus::zvariant::DynamicType,
    >(
        &self,
        method: &str,
        body: &B,
    ) -> Result<R> {
        let json: String = self.proxy().await?.call(method, body).await?;
        serde_json::from_str(&json).with_context(|| format!("invalid {method} response"))
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("massiveeqctl: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let command = Command::parse(std::env::args().skip(1)).map_err(|error| anyhow!(error))?;
    match command {
        Command::Status { watch: false } => {
            let snapshot = match Client::connect().await {
                Ok(client) => client
                    .snapshot()
                    .await
                    .unwrap_or_else(QuickSnapshot::offline),
                Err(error) => QuickSnapshot::offline(error),
            };
            emit(&snapshot)?;
        }
        Command::Status { watch: true } => watch().await?,
        Command::Engine { enabled } => {
            Client::connect().await?.set_global_bypass(!enabled).await?;
        }
        Command::Filters {
            device_key,
            enabled,
        } => {
            Client::connect()
                .await?
                .set_device_bypass(&device_key, !enabled)
                .await?;
        }
        Command::Assign {
            device_key,
            profile_id,
        } => {
            Client::connect()
                .await?
                .assign(&device_key, &profile_id)
                .await?
        }
        Command::Unassign { device_key } => {
            Client::connect().await?.assign(&device_key, "").await?
        }
        Command::Compare {
            device_key,
            profile_id,
        } => {
            Client::connect()
                .await?
                .compare(&device_key, &profile_id)
                .await?
        }
        Command::SetFilter {
            profile_id,
            filter_index,
            frequency_hz,
            gain_db,
            q,
        } => {
            Client::connect()
                .await?
                .set_filter(&profile_id, filter_index, frequency_hz, gain_db, q)
                .await?
        }
    }
    Ok(())
}

async fn watch() -> Result<()> {
    let mut last_line = None;
    loop {
        match Client::connect().await {
            Ok(client) => {
                if let Err(error) = watch_connection(&client, &mut last_line).await {
                    emit_if_changed(QuickSnapshot::offline(error), &mut last_line)?;
                }
            }
            Err(error) => emit_if_changed(QuickSnapshot::offline(error), &mut last_line)?,
        }
        sleep(Duration::from_secs(2)).await;
    }
}

async fn watch_connection(client: &Client, last_line: &mut Option<String>) -> Result<()> {
    let proxy = client.proxy().await?;
    let mut changed = proxy
        .receive_signal("Changed")
        .await
        .context("could not subscribe to MassiveEQ changes")?;
    let mut owner_changed = proxy
        .receive_owner_changed()
        .await
        .context("could not monitor the MassiveEQ service")?;

    emit_if_changed(client.snapshot().await?, last_line)?;
    loop {
        tokio::select! {
            signal = changed.next() => {
                if signal.is_none() {
                    return Err(anyhow!("MassiveEQ change stream ended"));
                }
                match client.snapshot().await {
                    Ok(snapshot) => emit_if_changed(snapshot, last_line)?,
                    Err(error) => return Err(error),
                }
            }
            owner = owner_changed.next() => {
                match owner {
                    Some(Some(_)) => {
                        match client.snapshot().await {
                            Ok(snapshot) => emit_if_changed(snapshot, last_line)?,
                            Err(error) => return Err(error),
                        }
                    }
                    Some(None) => return Err(anyhow!("MassiveEQ audio service stopped")),
                    None => return Err(anyhow!("MassiveEQ owner stream ended")),
                }
            }
        }
    }
}

fn emit(snapshot: &QuickSnapshot) -> Result<()> {
    let line = serde_json::to_string(snapshot).context("could not encode status")?;
    println!("{line}");
    std::io::stdout().flush().context("could not flush status")
}

fn emit_if_changed(snapshot: QuickSnapshot, last_line: &mut Option<String>) -> Result<()> {
    let line = serde_json::to_string(&snapshot).context("could not encode status")?;
    if last_line.as_deref() != Some(&line) {
        println!("{line}");
        std::io::stdout()
            .flush()
            .context("could not flush status")?;
        *last_line = Some(line);
    }
    Ok(())
}
