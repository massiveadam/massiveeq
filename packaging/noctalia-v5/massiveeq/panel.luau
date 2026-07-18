--!nonstrict

local snapshot = noctalia.state.get("snapshot") or {
  schema_version = 1,
  online = false,
  state = "unavailable",
  global_bypass = false,
  profiles = {},
  outputs = {},
  error = "Connecting to MassiveEQ…",
  loading = true,
}

local selectedOutputKey = nil
local manualOutputSelection = false
local outputChoiceKeys = {}
local profileChoiceIds = {}
local comparisonChoiceIds = {}
local activeFilters = {}
local activeProfileId = nil
local pendingAction = false
local actionError = ""

local render

local function shellQuote(value)
  return "'" .. tostring(value):gsub("'", "'\\''") .. "'"
end

local function requestRefresh()
  local nonce = tonumber(noctalia.state.get("refresh_requested")) or 0
  noctalia.state.set("refresh_requested", nonce + 1)
end

local function runAction(arguments)
  if pendingAction then
    return
  end

  local parts = { "massiveeqctl" }
  for _, argument in ipairs(arguments) do
    table.insert(parts, shellQuote(argument))
  end

  pendingAction = true
  actionError = ""
  render()
  local started = noctalia.runAsync(table.concat(parts, " "), function(result)
    pendingAction = false
    if result == nil or result.exitCode ~= 0 then
      local detail = result and noctalia.string.trim(result.stderr or "") or ""
      actionError = detail ~= "" and detail or "MassiveEQ rejected the change"
      render()
      return
    end
    requestRefresh()
    render()
  end, 10000)

  if not started then
    pendingAction = false
    actionError = "Could not start MassiveEQ control"
    render()
  end
end

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

local function profileById(profileId)
  for _, profile in ipairs(snapshot.profiles or {}) do
    if profile.id == profileId then
      return profile
    end
  end
  return nil
end

local function connectedOutputs()
  local outputs = {}
  for _, output in ipairs(snapshot.outputs or {}) do
    if output.connected then
      table.insert(outputs, output)
    end
  end
  return outputs
end

local function preferredOutput(outputs)
  local defaultNodeName = snapshot.default_output_node_name
  if defaultNodeName ~= nil and defaultNodeName ~= "" then
    for _, output in ipairs(outputs) do
      if output.node_name == defaultNodeName then
        return output
      end
    end
  end
  for _, output in ipairs(outputs) do
    if output.assigned_profile_id ~= nil and output.assigned_profile_id ~= "" and not output.bypassed then
      return output
    end
  end
  for _, output in ipairs(outputs) do
    if output.assigned_profile_id ~= nil and output.assigned_profile_id ~= "" then
      return output
    end
  end
  return outputs[1]
end

local function selectedOutput()
  local outputs = connectedOutputs()
  if manualOutputSelection then
    for _, output in ipairs(outputs) do
      if output.key == selectedOutputKey then
        return output
      end
    end
    manualOutputSelection = false
  end
  local output = preferredOutput(outputs)
  selectedOutputKey = output and output.key or nil
  return output
end

local function activeProfile(output)
  if output == nil then
    return nil
  end
  local comparison = output.comparison
  if comparison ~= nil and comparison.enabled and comparison.active_profile_id ~= "__bypass__" then
    return profileById(comparison.active_profile_id)
  end
  return profileById(output.assigned_profile_id or "")
end

local function profileName(profileId)
  if profileId == "__bypass__" then
    return "Off · level matched"
  end
  local profile = profileById(profileId)
  return profile and profile.name or "Missing profile"
end

local function card(children)
  return ui.column({
    fill = "surface_variant/0.38",
    radius = 12,
    padding = 12,
    gap = 10,
    align = "stretch",
  }, children)
end

local function outputSelector(outputs, output)
  outputChoiceKeys = {}
  local options = {}
  local selectedIndex = 0
  for index, candidate in ipairs(outputs) do
    table.insert(outputChoiceKeys, candidate.key)
    table.insert(options, candidate.description)
    if candidate.key == output.key then
      selectedIndex = index - 1
    end
  end

  if #outputs == 1 then
    return ui.row({ gap = 8, align = "center" }, {
      ui.glyph({ name = "device-speaker", size = 17, color = "primary" }),
      ui.label({ text = output.description, fontWeight = "medium", maxLines = 1, flexGrow = 1 }),
    })
  end

  return ui.row({ gap = 8, align = "center" }, {
    ui.glyph({ name = "device-speaker", size = 17, color = "primary" }),
    ui.select({
      key = "output-selector",
      options = options,
      selectedIndex = selectedIndex,
      onChange = "onOutputChanged",
      controlSize = "sm",
      enabled = not pendingAction,
      flexGrow = 1,
    }),
  })
end

local function profileSelector(output)
  profileChoiceIds = { "" }
  local options = { "Unassigned" }
  local selectedIndex = 0

  for _, profile in ipairs(snapshot.profiles or {}) do
    table.insert(profileChoiceIds, profile.id)
    local label = profile.name
    if not profile.activatable then
      label = label .. " · unavailable"
    end
    table.insert(options, label)
    if profile.id == output.assigned_profile_id then
      selectedIndex = #options - 1
    end
  end

  return ui.select({
    key = "profile-selector-" .. output.key,
    options = options,
    selectedIndex = selectedIndex,
    onChange = "onProfileChanged",
    controlSize = "sm",
    enabled = not pendingAction,
  })
end

local function comparisonControls(output)
  local comparison = output.comparison
  comparisonChoiceIds = {}
  if comparison == nil or not comparison.enabled then
    return nil
  end

  local rows = {}
  local buttons = {}
  for index, profileId in ipairs(comparison.profile_ids or {}) do
    if index > 9 then
      break
    end
    table.insert(comparisonChoiceIds, profileId)
    table.insert(buttons, ui.button({
      key = "comparison-" .. profileId,
      text = profileName(profileId),
      selected = comparison.active_profile_id == profileId,
      variant = comparison.active_profile_id == profileId and "primary" or "outline",
      controlSize = "sm",
      enabled = not pendingAction,
      onClick = "onComparison" .. tostring(index),
    }))
    if #buttons == 3 then
      table.insert(rows, ui.row({ gap = 6, align = "center" }, buttons))
      buttons = {}
    end
  end
  if #buttons > 0 then
    table.insert(rows, ui.row({ gap = 6, align = "center" }, buttons))
  end

  local contents = {
    ui.label({ text = "LEVEL-MATCHED COMPARISON", fontSize = 10, fontWeight = "bold", color = "on_surface_variant" }),
  }
  for _, row in ipairs(rows) do
    table.insert(contents, row)
  end
  return ui.column({ gap = 6, align = "stretch" }, contents)
end

local function filterGlyph(kind)
  if kind == "LPQ" or kind == "HPQ" or kind == "BP" then
    return "filter"
  elseif kind == "NO" then
    return "cut"
  elseif kind == "LSC" or kind == "HSC" then
    return "chart-line"
  end
  return "wave-sine"
end

local function formatFrequency(value)
  if value >= 1000 then
    return string.format("%.0f", value)
  end
  return string.format("%.1f", value)
end

local function filterRow(profile, filter, slot)
  local identity = profile.id .. "-" .. tostring(filter.index)
  return ui.row({ key = identity, gap = 5, align = "center" }, {
    ui.column({ width = 30, gap = 1, align = "center" }, {
      ui.glyph({ name = filterGlyph(filter.kind), size = 17, color = filter.enabled and "primary" or "on_surface_variant" }),
      ui.label({ text = filter.kind, fontSize = 8, color = "on_surface_variant", textAlign = "center" }),
    }),
    ui.input({
      key = identity .. "-frequency-" .. tostring(filter.frequency_hz),
      value = formatFrequency(filter.frequency_hz),
      width = 78,
      controlSize = "sm",
      fontSize = 12,
      enabled = filter.enabled and not pendingAction,
      onSubmit = "onFrequency" .. tostring(slot),
    }),
    ui.label({ text = "Hz", fontSize = 10, color = "on_surface_variant" }),
    ui.input({
      key = identity .. "-gain-" .. tostring(filter.gain_db),
      value = string.format("%.1f", filter.gain_db),
      width = 66,
      controlSize = "sm",
      fontSize = 12,
      enabled = filter.enabled and not pendingAction,
      onSubmit = "onGain" .. tostring(slot),
    }),
    ui.label({ text = "dB", fontSize = 10, color = "on_surface_variant" }),
    ui.input({
      key = identity .. "-q-" .. tostring(filter.q),
      value = string.format("%.2f", filter.q),
      width = 60,
      controlSize = "sm",
      fontSize = 12,
      enabled = filter.enabled and not pendingAction,
      onSubmit = "onQ" .. tostring(slot),
    }),
    ui.label({ text = "Q", fontSize = 10, color = "on_surface_variant" }),
  })
end

local function filtersSection(output, profile)
  activeProfileId = profile and profile.id or nil
  activeFilters = {}

  local contents = {
    ui.row({ align = "center", justify = "space_between", gap = 8 }, {
      ui.column({ gap = 1, flexGrow = 1 }, {
        ui.label({ text = "Filters", fontSize = 15, fontWeight = "medium" }),
        ui.label({
          text = output.bypassed and "Level-matched bypass" or "EQ processing enabled",
          fontSize = 11,
          color = "on_surface_variant",
        }),
      }),
      ui.toggle({
        checked = not output.bypassed,
        enabled = not pendingAction,
        onChange = "onFiltersChanged",
      }),
    }),
  }

  if profile ~= nil and #(profile.filters or {}) > 0 then
    table.insert(contents, ui.separator({ spacing = 2 }))
    table.insert(contents, ui.row({ align = "center", justify = "space_between", gap = 8 }, {
      ui.row({ gap = 7, align = "center", flexGrow = 1 }, {
        ui.glyph({ name = "adjustments-horizontal", size = 16, color = "primary" }),
        ui.label({ text = profile.name, fontWeight = "medium", maxLines = 1 }),
        ui.label({ text = tostring(profile.band_count) .. " bands", fontSize = 10, color = "on_surface_variant" }),
      }),
      ui.button({ text = "Edit", variant = "outline", controlSize = "sm", onClick = "onOpenEditor" }),
    }))

    for slot, filter in ipairs(profile.filters) do
      if slot > 10 then
        break
      end
      table.insert(activeFilters, filter)
      table.insert(contents, filterRow(profile, filter, slot))
    end
  elseif profile == nil then
    table.insert(contents, ui.label({
      text = "Assign a parametric profile to expose quick filter controls.",
      fontSize = 11,
      color = "on_surface_variant",
    }))
  else
    table.insert(contents, ui.label({
      text = "This profile has no editable parametric filters.",
      fontSize = 11,
      color = "on_surface_variant",
    }))
  end

  return card(contents)
end

local function sourceSection(outputs, output)
  local contents = {
    outputSelector(outputs, output),
    profileSelector(output),
  }
  local comparison = comparisonControls(output)
  if comparison ~= nil then
    table.insert(contents, comparison)
  end
  return card(contents)
end

local function statusCard(glyph, message, color, includeRetry)
  local children = {
    ui.glyph({ name = glyph, size = 20, color = color }),
    ui.label({ text = message, color = "on_surface_variant", maxLines = 3, flexGrow = 1 }),
  }
  if includeRetry then
    table.insert(children, ui.button({ text = "Retry", variant = "outline", controlSize = "sm", onClick = "onRetry" }))
  end
  return card({ ui.row({ gap = 10, align = "center" }, children) })
end

render = function()
  local body = {}
  local outputs = connectedOutputs()
  local output = selectedOutput()

  if snapshot.loading then
    table.insert(body, statusCard("loader", "Connecting to MassiveEQ…", "primary", false))
  elseif not snapshot.online then
    table.insert(body, statusCard("alert-circle", snapshot.error or "MassiveEQ is unavailable", "error", true))
  elseif #outputs == 0 or output == nil then
    table.insert(body, statusCard("device-speaker-off", "No connected playback outputs", "on_surface_variant", true))
  else
    table.insert(body, sourceSection(outputs, output))
    table.insert(body, filtersSection(output, activeProfile(output)))
  end

  if actionError ~= "" then
    table.insert(body, statusCard("alert-triangle", actionError, "error", true))
  end

  panel.render(ui.column({ flexGrow = 1, gap = 10, align = "stretch" }, {
    ui.row({ align = "center", gap = 8 }, {
      ui.glyph({ name = "wave-sine", size = 20, color = snapshot.online and "primary" or "error" }),
      ui.column({ gap = 0 }, {
        ui.label({ text = "MASSIVE / EQ", fontSize = 14, fontWeight = "bold" }),
        ui.label({ text = stateLabel(), fontSize = 10, color = "on_surface_variant" }),
      }),
      ui.spacer({ flexGrow = 1 }),
      ui.label({ text = "Engine", fontSize = 13, fontWeight = "medium" }),
      ui.toggle({
        checked = snapshot.online and not snapshot.global_bypass,
        enabled = snapshot.online and not pendingAction,
        onChange = "onEngineChanged",
      }),
      ui.button({ glyph = "close", variant = "ghost", onClick = "onCloseClicked" }),
    }),
    ui.scroll({ flexGrow = 1, gap = 10, align = "stretch" }, body),
    ui.button({
      text = "Open full editor",
      glyph = "external-link",
      variant = "primary",
      contentAlign = "center",
      controlSize = "md",
      onClick = "onOpenEditor",
    }),
  }))
end

local function parseField(value)
  return tonumber((tostring(value):gsub("[^%d%.%+%-]", "")))
end

local function commitFilter(slot, field, value)
  local filter = activeFilters[slot]
  if filter == nil or activeProfileId == nil then
    actionError = "That filter is no longer available"
    render()
    return
  end

  local parsed = parseField(value)
  local frequency = filter.frequency_hz
  local gain = filter.gain_db
  local q = filter.q
  if field == "frequency" then
    if parsed == nil or parsed < 20 or parsed > 20000 then
      actionError = "Frequency must be between 20 and 20000 Hz"
      render()
      return
    end
    frequency = parsed
  elseif field == "gain" then
    if parsed == nil or parsed < -60 or parsed > 60 then
      actionError = "Gain must be between -60 and 60 dB"
      render()
      return
    end
    gain = parsed
  else
    if parsed == nil or parsed < 0.01 or parsed > 1000 then
      actionError = "Q must be between 0.01 and 1000"
      render()
      return
    end
    q = parsed
  end

  runAction({ "set-filter", activeProfileId, filter.index, frequency, gain, q })
end

function onOpen(_context)
  manualOutputSelection = false
  requestRefresh()
  render()
end

function onCloseClicked()
  panel.close()
end

function onOpenEditor()
  if not noctalia.runAsync("massiveeq") then
    actionError = "Could not open the full editor"
    render()
  end
end

function onRetry()
  actionError = ""
  requestRefresh()
  render()
end

function onEngineChanged(value)
  runAction({ "engine", value == "true" and "on" or "off" })
end

function onFiltersChanged(value)
  local output = selectedOutput()
  if output ~= nil then
    runAction({ "filters", output.key, value == "true" and "on" or "off" })
  end
end

function onOutputChanged(index, _label)
  local position = (tonumber(index) or 0) + 1
  selectedOutputKey = outputChoiceKeys[position]
  manualOutputSelection = selectedOutputKey ~= nil
  actionError = ""
  render()
end

function onProfileChanged(index, _label)
  local output = selectedOutput()
  local position = (tonumber(index) or 0) + 1
  local profileId = profileChoiceIds[position]
  if output == nil or profileId == nil then
    return
  end
  if profileId == "" then
    runAction({ "unassign", output.key })
    return
  end
  local profile = profileById(profileId)
  if profile == nil or not profile.activatable then
    actionError = "That profile has errors and cannot be activated"
    render()
    return
  end
  runAction({ "assign", output.key, profileId })
end

local function selectComparison(slot)
  local output = selectedOutput()
  local profileId = comparisonChoiceIds[slot]
  if output ~= nil and profileId ~= nil then
    runAction({ "compare", output.key, profileId })
  end
end

function onComparison1() selectComparison(1) end
function onComparison2() selectComparison(2) end
function onComparison3() selectComparison(3) end
function onComparison4() selectComparison(4) end
function onComparison5() selectComparison(5) end
function onComparison6() selectComparison(6) end
function onComparison7() selectComparison(7) end
function onComparison8() selectComparison(8) end
function onComparison9() selectComparison(9) end

function onFrequency1(value) commitFilter(1, "frequency", value) end
function onFrequency2(value) commitFilter(2, "frequency", value) end
function onFrequency3(value) commitFilter(3, "frequency", value) end
function onFrequency4(value) commitFilter(4, "frequency", value) end
function onFrequency5(value) commitFilter(5, "frequency", value) end
function onFrequency6(value) commitFilter(6, "frequency", value) end
function onFrequency7(value) commitFilter(7, "frequency", value) end
function onFrequency8(value) commitFilter(8, "frequency", value) end
function onFrequency9(value) commitFilter(9, "frequency", value) end
function onFrequency10(value) commitFilter(10, "frequency", value) end

function onGain1(value) commitFilter(1, "gain", value) end
function onGain2(value) commitFilter(2, "gain", value) end
function onGain3(value) commitFilter(3, "gain", value) end
function onGain4(value) commitFilter(4, "gain", value) end
function onGain5(value) commitFilter(5, "gain", value) end
function onGain6(value) commitFilter(6, "gain", value) end
function onGain7(value) commitFilter(7, "gain", value) end
function onGain8(value) commitFilter(8, "gain", value) end
function onGain9(value) commitFilter(9, "gain", value) end
function onGain10(value) commitFilter(10, "gain", value) end

function onQ1(value) commitFilter(1, "q", value) end
function onQ2(value) commitFilter(2, "q", value) end
function onQ3(value) commitFilter(3, "q", value) end
function onQ4(value) commitFilter(4, "q", value) end
function onQ5(value) commitFilter(5, "q", value) end
function onQ6(value) commitFilter(6, "q", value) end
function onQ7(value) commitFilter(7, "q", value) end
function onQ8(value) commitFilter(8, "q", value) end
function onQ9(value) commitFilter(9, "q", value) end
function onQ10(value) commitFilter(10, "q", value) end

noctalia.state.watch("snapshot", function(value)
  snapshot = value
  render()
end)
