import QtQuick
import Quickshell
import Quickshell.Io

Item {
  id: root

  property var pluginApi: null
  property var snapshot: ({
                            "schema_version": 1,
                            "online": false,
                            "state": "unavailable",
                            "global_bypass": false,
                            "profiles": [],
                            "outputs": [],
                            "error": "Waiting for MassiveEQ"
                          })
  property string actionError: ""
  property int pendingActions: 0
  property bool loading: true
  readonly property bool actionBusy: pendingActions > 0

  function acceptSnapshot(line) {
    const text = String(line || "").trim();
    if (!text)
      return;
    try {
      const next = JSON.parse(text);
      if (next.schema_version !== 1) {
        markUnavailable("Unsupported MassiveEQ status format");
        return;
      }
      snapshot = next;
      loading = false;
      if (next.online)
        actionError = "";
    } catch (error) {
      markUnavailable("Could not read MassiveEQ status");
    }
  }

  function markUnavailable(message) {
    loading = false;
    snapshot = {
      "schema_version": 1,
      "online": false,
      "state": "unavailable",
      "global_bypass": false,
      "profiles": [],
      "outputs": [],
      "error": message || "MassiveEQ is unavailable"
    };
  }

  function retry() {
    actionError = "";
    loading = true;
    statusWatcher.running = false;
    restartTimer.restart();
  }

  function runAction(args) {
    actionError = "";
    const process = actionProcess.createObject(root, {
                                                 "command": ["massiveeqctl"].concat(args)
                                               });
    if (!process) {
      actionError = "Could not start MassiveEQ control";
      return;
    }
    pendingActions++;
    process.exited.connect(function (exitCode) {
      pendingActions = Math.max(0, pendingActions - 1);
      if (exitCode !== 0) {
        const detail = String(process.capturedError || "").trim();
        actionError = detail || "MassiveEQ rejected the change";
      }
      process.destroy();
    });
    process.running = true;
  }

  function setEngine(enabled) {
    runAction(["engine", enabled ? "on" : "off"]);
  }

  function setFilters(deviceKey, enabled) {
    runAction(["filters", deviceKey, enabled ? "on" : "off"]);
  }

  function assignProfile(deviceKey, profileId) {
    if (profileId === "")
      runAction(["unassign", deviceKey]);
    else
      runAction(["assign", deviceKey, profileId]);
  }

  function selectComparison(deviceKey, profileId) {
    runAction(["compare", deviceKey, profileId]);
  }

  function setProfileFilter(profileId, filterIndex, frequencyHz, gainDb, q) {
    runAction([
                "set-filter",
                profileId,
                String(filterIndex),
                String(frequencyHz),
                String(gainDb),
                String(q)
              ]);
  }

  function openFullApp() {
    Quickshell.execDetached(["massiveeq"]);
  }

  function profileName(profileId) {
    if (profileId === "__bypass__")
      return "Off · level matched";
    const profiles = snapshot.profiles || [];
    for (let i = 0; i < profiles.length; i++) {
      if (profiles[i].id === profileId)
        return profiles[i].name;
    }
    return "Missing profile";
  }

  function profileInfo(profileId) {
    const profiles = snapshot.profiles || [];
    for (let index = 0; index < profiles.length; index++) {
      if (profiles[index].id === profileId)
        return profiles[index];
    }
    return null;
  }

  function activeProfileForOutput(output) {
    const comparison = output?.comparison;
    if (comparison?.enabled && comparison.active_profile_id !== "__bypass__")
      return profileInfo(comparison.active_profile_id);
    return profileInfo(output?.assigned_profile_id || "");
  }

  function stateLabel() {
    switch (snapshot.state) {
    case "active":
      return "Active";
    case "idle":
      return "Idle";
    case "engine_off":
      return "Engine off";
    default:
      return "Unavailable";
    }
  }

  Process {
    id: statusWatcher
    command: ["massiveeqctl", "status", "--watch"]
    running: true
    stdout: SplitParser {
      onRead: line => root.acceptSnapshot(line)
    }
    stderr: StdioCollector {
      id: watcherError
    }
    onExited: (exitCode, exitStatus) => {
                const detail = String(watcherError.text || "").trim();
                root.markUnavailable(detail || "MassiveEQ status helper stopped");
                restartTimer.restart();
              }
  }

  Timer {
    id: restartTimer
    interval: 1500
    repeat: false
    onTriggered: statusWatcher.running = true
  }

  Component {
    id: actionProcess
    Process {
      property string capturedError: errorCollector.text
      stderr: StdioCollector {
        id: errorCollector
      }
    }
  }
}
