--!nonstrict
-- Poll the desktop-neutral helper and publish its normalized snapshot to the
-- widget and panel. Polling deliberately survives daemon replacement and a
-- stopped/restarted D-Bus service without owning any audio state itself.

local polling = false

local function offline(message, loading)
  return {
    schema_version = 1,
    online = false,
    state = "unavailable",
    global_bypass = false,
    profiles = {},
    outputs = {},
    error = message,
    loading = loading == true,
  }
end

if noctalia.state.get("snapshot") == nil then
  noctalia.state.set("snapshot", offline("Connecting to MassiveEQ…", true))
end

local function publishResult(result)
  if result == nil or result.exitCode ~= 0 then
    polling = false
    local detail = result and noctalia.string.trim(result.stderr or "") or ""
    noctalia.state.set("snapshot", offline(detail ~= "" and detail or "Could not run massiveeqctl", false))
    return
  end

  local value, decodeError = noctalia.json.decode(noctalia.string.trim(result.stdout or ""))
  if value == nil then
    polling = false
    noctalia.state.set("snapshot", offline("Could not read MassiveEQ status: " .. tostring(decodeError or "invalid JSON"), false))
    return
  end
  if value.schema_version ~= 1 then
    polling = false
    noctalia.state.set("snapshot", offline("Unsupported MassiveEQ status format", false))
    return
  end

  value.loading = false
  if not noctalia.commandExists("wpctl") then
    polling = false
    noctalia.state.set("snapshot", value)
    return
  end

  -- Noctalia v5 deliberately keeps its plugin audio API small. Ask
  -- WirePlumber which sink is currently routed so the panel can select that
  -- output instead of displaying every detected sound device.
  local started = noctalia.runAsync("wpctl inspect @DEFAULT_AUDIO_SINK@", function(inspectResult)
    polling = false
    if inspectResult ~= nil and inspectResult.exitCode == 0 then
      value.default_output_node_name = (inspectResult.stdout or ""):match('node%.name%s*=%s*"([^"]+)"')
    end
    noctalia.state.set("snapshot", value)
  end, 3000)
  if not started then
    polling = false
    noctalia.state.set("snapshot", value)
  end
end

local function poll()
  if polling then
    return
  end
  if not noctalia.commandExists("massiveeqctl") then
    noctalia.state.set("snapshot", offline("massiveeqctl is not installed", false))
    return
  end

  polling = true
  if not noctalia.runAsync("massiveeqctl status", publishResult, 5000) then
    polling = false
    noctalia.state.set("snapshot", offline("Could not start massiveeqctl", false))
  end
end

noctalia.setUpdateInterval(1000)
noctalia.state.watch("refresh_requested", function(_value)
  poll()
end)

function update()
  poll()
end
