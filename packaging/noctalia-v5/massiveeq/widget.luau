--!nonstrict
-- The host owns the widget capsule and its hover state. Keeping this entry on
-- the imperative glyph API avoids custom pointer layers and works on every bar
-- orientation.

local PANEL_ID = "massiveeq/massiveeq:panel"
local snapshot = noctalia.state.get("snapshot") or {
  online = false,
  state = "unavailable",
  error = "Connecting to MassiveEQ…",
  loading = true,
}

local function stateLabel()
  if snapshot.state == "active" then
    return "Active"
  elseif snapshot.state == "idle" then
    return "Idle"
  elseif snapshot.state == "engine_off" then
    return "Engine off"
  end
  return "Unavailable"
end

local function render()
  barWidget.setText("")
  barWidget.setGlyph("wave-sine")

  if snapshot.state == "active" then
    barWidget.setGlyphColor("primary")
  elseif snapshot.state == "engine_off" then
    barWidget.setGlyphColor("on_surface_variant")
  elseif snapshot.state == "unavailable" then
    barWidget.setGlyphColor("error")
  else
    barWidget.setGlyphColor("on_surface")
  end

  local detail = stateLabel()
  if not snapshot.online and snapshot.error ~= nil and snapshot.error ~= "" then
    detail = detail .. " — " .. snapshot.error
  end
  barWidget.setTooltip("MassiveEQ — " .. detail)
end

noctalia.state.watch("snapshot", function(value)
  snapshot = value
  render()
end)

function update()
  noctalia.setUpdateInterval(5000)
  render()
end

function onClick()
  noctalia.togglePanel(PANEL_ID)
end

function onRightClick()
  if not noctalia.runAsync("massiveeq") then
    noctalia.notifyError("MassiveEQ", "Could not open the full editor")
  end
end
