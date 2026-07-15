use anyhow::{Context, Result};
use ksni::{Status, ToolTip, TrayMethods};
use massiveeq_core::{DeviceInfo, ProfileInfo};
use tokio::sync::mpsc::{self, UnboundedSender};
use zbus::{Connection, Proxy};

const DESTINATION: &str = "org.massiveeq.Service1";
const OBJECT_PATH: &str = "/org/massiveeq/Service1";
const INTERFACE: &str = "org.massiveeq.Service1";

#[derive(Debug)]
enum Action {
    Open,
    ToggleGlobalBypass,
    SetDeviceBypass {
        key: String,
        bypassed: bool,
    },
    Assign {
        key: String,
        profile: Option<String>,
    },
    Refresh,
    Quit,
}

#[derive(Clone, Default)]
struct Snapshot {
    online: bool,
    global_bypass: bool,
    profiles: Vec<ProfileInfo>,
    devices: Vec<DeviceInfo>,
    error: Option<String>,
}

struct MassiveEqTray {
    snapshot: Snapshot,
    actions: UnboundedSender<Action>,
}

impl MassiveEqTray {
    fn send(&self, action: Action) {
        let _ = self.actions.send(action);
    }

    fn state_label(&self) -> &'static str {
        if !self.snapshot.online {
            "Unavailable"
        } else if self.snapshot.global_bypass {
            "Bypassed"
        } else if self
            .snapshot
            .devices
            .iter()
            .any(|device| device.connected && device.assigned_profile.is_some() && !device.bypassed)
        {
            "Active"
        } else {
            "Idle"
        }
    }

    fn device_menu(&self, device: &DeviceInfo) -> ksni::MenuItem<Self> {
        use ksni::menu::{CheckmarkItem, MenuItem, RadioGroup, RadioItem, SubMenu};

        let key = device.key.as_storage_key();
        let mut profile_ids = Vec::with_capacity(self.snapshot.profiles.len() + 1);
        profile_ids.push(None);
        profile_ids.extend(
            self.snapshot
                .profiles
                .iter()
                .map(|profile| Some(profile.id.clone())),
        );
        let selected = device
            .assigned_profile
            .as_ref()
            .and_then(|assigned| {
                self.snapshot
                    .profiles
                    .iter()
                    .position(|profile| &profile.id == assigned)
            })
            .map_or(0, |index| index + 1);

        let options = std::iter::once(RadioItem {
            label: "Unassigned (leave output unchanged)".into(),
            icon_name: "audio-volume-muted-symbolic".into(),
            ..Default::default()
        })
        .chain(self.snapshot.profiles.iter().map(|profile| RadioItem {
            label: profile.name.clone(),
            enabled: profile.activatable,
            icon_name: "audio-x-generic-symbolic".into(),
            ..Default::default()
        }))
        .collect();

        let assignment_key = key.clone();
        let assignment_ids = profile_ids;
        let bypass_key = key;
        let bypassed = device.bypassed;
        SubMenu {
            label: if device.connected {
                device.description.clone()
            } else {
                format!("{} (disconnected)", device.description)
            },
            icon_name: "audio-headphones-symbolic".into(),
            submenu: vec![
                RadioGroup {
                    selected,
                    options,
                    select: Box::new(move |tray: &mut Self, index| {
                        if let Some(profile) = assignment_ids.get(index).cloned() {
                            tray.send(Action::Assign {
                                key: assignment_key.clone(),
                                profile,
                            });
                        }
                    }),
                }
                .into(),
                MenuItem::Separator,
                CheckmarkItem {
                    label: "Bypass this output".into(),
                    checked: bypassed,
                    activate: Box::new(move |tray: &mut Self| {
                        tray.send(Action::SetDeviceBypass {
                            key: bypass_key.clone(),
                            bypassed: !bypassed,
                        });
                    }),
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        }
        .into()
    }
}

impl ksni::Tray for MassiveEqTray {
    fn id(&self) -> String {
        "massiveeq-tray".into()
    }

    fn title(&self) -> String {
        format!("MassiveEQ — {}", self.state_label())
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }

    fn status(&self) -> Status {
        if self.snapshot.online {
            Status::Active
        } else {
            Status::NeedsAttention
        }
    }

    fn icon_name(&self) -> String {
        if !self.snapshot.online {
            "dialog-error-symbolic".into()
        } else if self.snapshot.global_bypass {
            "audio-volume-muted-symbolic".into()
        } else {
            "org.massiveeq.MassiveEQ".into()
        }
    }

    fn attention_icon_name(&self) -> String {
        "dialog-error-symbolic".into()
    }

    fn tool_tip(&self) -> ToolTip {
        let description = if let Some(error) = &self.snapshot.error {
            error.clone()
        } else {
            let active = self
                .snapshot
                .devices
                .iter()
                .filter(|device| device.connected)
                .map(|device| {
                    let profile = device
                        .assigned_profile
                        .as_ref()
                        .and_then(|id| self.snapshot.profiles.iter().find(|p| &p.id == id))
                        .map_or("Unassigned", |profile| profile.name.as_str());
                    format!("{} — {profile}", device.description)
                })
                .collect::<Vec<_>>();
            if active.is_empty() {
                "No connected outputs".into()
            } else {
                active.join("\n")
            }
        };
        ToolTip {
            icon_name: self.icon_name(),
            title: self.title(),
            description,
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Action::Open);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        self.send(Action::ToggleGlobalBypass);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{CheckmarkItem, MenuItem, StandardItem};

        let mut menu = vec![
            StandardItem {
                label: format!("MassiveEQ — {}", self.state_label()),
                enabled: false,
                icon_name: self.icon_name(),
                ..Default::default()
            }
            .into(),
        ];

        if self.snapshot.online {
            if self.snapshot.devices.is_empty() {
                menu.push(
                    StandardItem {
                        label: "No playback outputs found".into(),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
            } else {
                menu.extend(
                    self.snapshot
                        .devices
                        .iter()
                        .map(|device| self.device_menu(device)),
                );
            }
            menu.push(MenuItem::Separator);
            menu.push(
                CheckmarkItem {
                    label: "Bypass all EQ".into(),
                    checked: self.snapshot.global_bypass,
                    activate: Box::new(|tray: &mut Self| {
                        tray.send(Action::ToggleGlobalBypass);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        } else {
            menu.push(
                StandardItem {
                    label: self
                        .snapshot
                        .error
                        .clone()
                        .unwrap_or_else(|| "Audio service is unavailable".into()),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
            menu.push(
                StandardItem {
                    label: "Retry connection".into(),
                    icon_name: "view-refresh-symbolic".into(),
                    activate: Box::new(|tray: &mut Self| tray.send(Action::Refresh)),
                    ..Default::default()
                }
                .into(),
            );
        }

        menu.extend([
            MenuItem::Separator,
            StandardItem {
                label: "Open MassiveEQ".into(),
                icon_name: "org.massiveeq.MassiveEQ".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Action::Open)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit tray icon".into(),
                icon_name: "application-exit-symbolic".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Action::Quit)),
                ..Default::default()
            }
            .into(),
        ]);
        menu
    }
}

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
            .await?;
        Ok(client)
    }

    async fn proxy(&self) -> Result<Proxy<'_>> {
        Proxy::new(&self.connection, DESTINATION, OBJECT_PATH, INTERFACE)
            .await
            .context("could not connect to MassiveEQ")
    }

    async fn snapshot(&self) -> Result<Snapshot> {
        let profiles: Vec<ProfileInfo> = self.call_json("ListProfiles", &()).await?;
        let devices: Vec<DeviceInfo> = self.call_json("ListDevices", &()).await?;
        let status: serde_json::Value = self.call_json("Status", &()).await?;
        Ok(Snapshot {
            online: true,
            global_bypass: status
                .get("global_bypass")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            profiles,
            devices,
            error: None,
        })
    }

    async fn assign(&self, key: &str, profile: Option<&str>) -> Result<()> {
        self.proxy()
            .await?
            .call("AssignProfile", &(key, profile.unwrap_or_default()))
            .await
            .context("could not update output assignment")
    }

    async fn set_device_bypass(&self, key: &str, bypassed: bool) -> Result<()> {
        self.proxy()
            .await?
            .call("SetDeviceBypass", &(key, bypassed))
            .await
            .context("could not update output bypass")
    }

    async fn set_global_bypass(&self, bypassed: bool) -> Result<()> {
        self.proxy()
            .await?
            .call("SetGlobalBypass", &(bypassed))
            .await
            .context("could not update global bypass")
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

fn offline_snapshot(error: impl std::fmt::Display) -> Snapshot {
    Snapshot {
        error: Some(error.to_string()),
        ..Default::default()
    }
}

async fn refresh(client: &mut Option<Client>) -> Snapshot {
    if client.is_none() {
        *client = Client::connect().await.ok();
    }
    match client.as_ref() {
        Some(active_client) => match active_client.snapshot().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                *client = None;
                offline_snapshot(error)
            }
        },
        None => offline_snapshot("MassiveEQ audio service is unavailable"),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let (actions, mut receiver) = mpsc::unbounded_channel();
    let mut client = Client::connect().await.ok();
    let initial = refresh(&mut client).await;
    let tray = MassiveEqTray {
        snapshot: initial,
        actions,
    };
    let handle = tray
        .assume_sni_available(true)
        .spawn()
        .await
        .context("could not register the MassiveEQ tray icon")?;
    let mut poll = tokio::time::interval(std::time::Duration::from_secs(3));

    loop {
        let action = tokio::select! {
            action = receiver.recv() => match action {
                Some(action) => action,
                None => break,
            },
            _ = poll.tick() => Action::Refresh,
            _ = tokio::signal::ctrl_c() => break,
        };

        let result = match action {
            Action::Open => {
                let _ = tokio::process::Command::new("massiveeq").spawn();
                continue;
            }
            Action::ToggleGlobalBypass => {
                if let Some(client) = client.as_ref() {
                    let current = client
                        .snapshot()
                        .await
                        .is_ok_and(|snapshot| snapshot.global_bypass);
                    client.set_global_bypass(!current).await
                } else {
                    Err(anyhow::anyhow!("MassiveEQ audio service is unavailable"))
                }
            }
            Action::SetDeviceBypass { key, bypassed } => {
                if let Some(client) = client.as_ref() {
                    client.set_device_bypass(&key, bypassed).await
                } else {
                    Err(anyhow::anyhow!("MassiveEQ audio service is unavailable"))
                }
            }
            Action::Assign { key, profile } => {
                if let Some(client) = client.as_ref() {
                    client.assign(&key, profile.as_deref()).await
                } else {
                    Err(anyhow::anyhow!("MassiveEQ audio service is unavailable"))
                }
            }
            Action::Refresh => Ok(()),
            Action::Quit => break,
        };

        let snapshot = if let Err(error) = result {
            offline_snapshot(error)
        } else {
            refresh(&mut client).await
        };
        let _ = handle
            .update(move |tray: &mut MassiveEqTray| {
                tray.snapshot = snapshot;
                true
            })
            .await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_is_unavailable() {
        let (actions, _) = mpsc::unbounded_channel();
        let tray = MassiveEqTray {
            snapshot: Snapshot::default(),
            actions,
        };
        assert_eq!(tray.state_label(), "Unavailable");
    }
}
