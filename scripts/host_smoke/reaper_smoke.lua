-- REAPER smoke test for the SDK example ("Example Gain"), one plug-in format per run.
-- Started by run.py through a separate REAPER instance with its own configuration.
-- Input comes from the environment; every path is inside the work root run.py made.
--   OPSMOKE_DIR     work directory of this run (results, renders, projects)
--   OPSMOKE_FORMAT  CLAP | VST3 | AU
--   OPSMOKE_WAV     input sine (stereo, 48 kHz)
-- Output: OPSMOKE_DIR/reaper_<FORMAT>.json, renders OPSMOKE_DIR/render_<FORMAT>_<tag>.wav

local dir = os.getenv("OPSMOKE_DIR")
local fmt = os.getenv("OPSMOKE_FORMAT")
local wav = os.getenv("OPSMOKE_WAV")
local r = reaper

local res = { format = fmt, checks = {}, reaper_version = r.GetAppVersion() }
local done = false

local function jenc(v)
  local t = type(v)
  if t == "nil" then return "null"
  elseif t == "boolean" then return v and "true" or "false"
  elseif t == "number" then
    if v ~= v or v == math.huge or v == -math.huge then return "null" end
    return string.format("%.9g", v)
  elseif t == "string" then
    return '"' .. v:gsub('[%c\\"]', function(c)
      if c == '"' then return '\\"' elseif c == "\\" then return "\\\\"
      elseif c == "\n" then return "\\n" else return string.format("\\u%04x", c:byte()) end
    end) .. '"'
  elseif t == "table" then
    if #v > 0 or next(v) == nil then
      local p = {}
      for i = 1, #v do p[i] = jenc(v[i]) end
      return "[" .. table.concat(p, ",") .. "]"
    end
    local p = {}
    for k, x in pairs(v) do p[#p + 1] = jenc(tostring(k)) .. ":" .. jenc(x) end
    return "{" .. table.concat(p, ",") .. "}"
  end
  return "null"
end

local function write_result()
  local f = io.open(dir .. "/reaper_" .. fmt .. ".json", "w")
  if f then f:write(jenc(res)); f:close() end
end

local function check(name, ok, detail)
  res.checks[#res.checks + 1] = { name = name, ok = ok and true or false, detail = detail or "" }
end

-- the steps run as a coroutine resumed from defer, so a step can wait for the UI
local function wait(sec)
  local t0 = r.time_precise()
  while r.time_precise() - t0 < sec do coroutine.yield() end
end

local function exists(path)
  local f = io.open(path, "rb")
  if f then f:close() end
  return f ~= nil
end

local function render(tag)
  r.GetSetProjectInfo_String(0, "RENDER_FILE", dir, true)
  r.GetSetProjectInfo_String(0, "RENDER_PATTERN", "render_" .. fmt .. "_" .. tag, true)
  r.GetSetProjectInfo(0, "RENDER_SETTINGS", 0, true)    -- master mix
  r.GetSetProjectInfo(0, "RENDER_BOUNDSFLAG", 1, true)  -- entire project
  r.GetSetProjectInfo(0, "RENDER_SRATE", 48000, true)
  r.GetSetProjectInfo(0, "RENDER_CHANNELS", 2, true)
  r.GetSetProjectInfo_String(0, "RENDER_FORMAT", "ZXZhdxgAAAA=", true) -- WAV, 24 bit
  r.Main_OnCommand(42230, 0) -- render with the most recent settings, close the dialog when done
  wait(0.5)
  return exists(dir .. "/render_" .. fmt .. "_" .. tag .. ".wav")
end

local function setp(tr, fx, p, norm)
  r.TrackFX_SetParamNormalized(tr, fx, p, norm)
  local _, txt = r.TrackFX_GetFormattedParamValue(tr, fx, p, "")
  return r.TrackFX_GetParamNormalized(tr, fx, p), txt
end

local function reopen(path)
  r.Main_OnCommand(40860, 0) -- close the current project tab
  r.Main_openProject("noprompt:" .. path)
  wait(1.0)
  return r.GetTrack(0, 0)
end

local function body()
  r.InsertTrackAtIndex(0, true)
  local tr = r.GetTrack(0, 0)
  r.SetOnlyTrackSelected(tr)
  r.SetEditCurPos(0, false, false)
  r.InsertMedia(wav, 0)
  check("track and input item", r.CountTrackMediaItems(tr) == 1, "items=" .. r.CountTrackMediaItems(tr))

  local prefix = ({ CLAP = "CLAP: ", VST3 = "VST3: ", AU = "AU: " })[fmt]
  -- the plug-in's own name is "Editor Gain" (examples/editor); the bundles are called "Example Gain"
  local fx = r.TrackFX_AddByName(tr, prefix .. "Editor Gain", false, 1)
  if fx < 0 then fx = r.TrackFX_AddByName(tr, prefix .. "Example Gain", false, 1) end
  local _, fxname = r.TrackFX_GetFXName(tr, fx, "")
  check("insert plug-in", fx >= 0, "fx index " .. tostring(fx) .. ", name '" .. tostring(fxname) .. "'")
  if fx < 0 then return end
  wait(0.5)

  local n = r.TrackFX_GetNumParams(tr, fx)
  local names, gp, wp = {}, nil, nil
  for i = 0, n - 1 do
    local _, nm = r.TrackFX_GetParamName(tr, fx, i, "")
    names[#names + 1] = nm
    if nm == "Gain" then gp = i end
    if nm == "Warm" then wp = i end
  end
  res.param_names = names
  check("parameters Gain and Warm", gp ~= nil and wp ~= nil, "count=" .. n .. " names=" .. table.concat(names, "|"))
  if not (gp and wp) then return end

  local _, t0 = setp(tr, fx, gp, 0.0)
  local _, t1 = setp(tr, fx, gp, 1.0)
  local _, td = setp(tr, fx, gp, 0.5)
  res.gain_text = { min = t0, max = t1, mid = td }
  local _, w0 = setp(tr, fx, wp, 0.0)
  local _, w1 = setp(tr, fx, wp, 1.0)
  res.warm_text = { off = w0, on = w1 }

  local g, t = setp(tr, fx, gp, 0.625) -- +6 dB
  check("set Gain", math.abs(g - 0.625) < 1e-3, "normalized " .. g .. ", text '" .. t .. "'")
  res.gain_6_text = t

  -- levels, rendered through the plug-in; run.py measures the files
  setp(tr, fx, wp, 0.0)
  setp(tr, fx, gp, 0.5);   check("render Gain 0 dB", render("g0"))
  setp(tr, fx, gp, 0.625); check("render Gain +6 dB", render("g6"))
  setp(tr, fx, gp, 0.25);  check("render Gain -12 dB", render("gm12"))
  setp(tr, fx, gp, 0.75);  check("render Gain +12 dB", render("g12"))
  local wv = setp(tr, fx, wp, 1.0)
  check("set Warm", wv > 0.99, "normalized " .. wv)
  check("render Gain +12 dB, Warm on", render("g12warm"))

  -- project 1: static values, save, reopen, compare
  local gb = setp(tr, fx, gp, 0.8125) -- +15 dB
  local wb = setp(tr, fx, wp, 1.0)
  local p1 = dir .. "/reaper_" .. fmt .. "_static.RPP"
  r.Main_SaveProjectEx(0, p1, 0)
  check("save project", exists(p1), p1)
  tr = reopen(p1)
  local ok2 = tr ~= nil and r.TrackFX_GetCount(tr) == 1
  check("reopen: plug-in present", ok2)
  if not ok2 then return end
  local g2 = r.TrackFX_GetParamNormalized(tr, 0, gp)
  local w2 = r.TrackFX_GetParamNormalized(tr, 0, wp)
  check("reopen: Gain and Warm kept", math.abs(g2 - gb) < 1e-3 and math.abs(w2 - wb) < 1e-3,
    string.format("gain %.6f -> %.6f, warm %.3f -> %.3f", gb, g2, wb, w2))
  local _, ft = r.TrackFX_GetFormattedParamValue(tr, 0, gp, "")
  res.reopen_gain_text = ft

  -- automation: Gain envelope from 0 dB (0.5) to +12 dB (0.75) over the clip
  setp(tr, 0, wp, 0.0)
  local env = r.GetFXEnvelope(tr, 0, gp, true)
  check("create Gain envelope", env ~= nil)
  if env then
    local len = r.GetProjectLength(0)
    r.InsertEnvelopePoint(env, 0.0, 0.5, 0, 0, false, true)
    r.InsertEnvelopePoint(env, len, 0.75, 0, 0, false, true)
    r.Envelope_SortPoints(env)
    check("render with Gain envelope", render("env"), "project length " .. len)
    local p2 = dir .. "/reaper_" .. fmt .. "_env.RPP"
    r.Main_SaveProjectEx(0, p2, 0)
    tr = reopen(p2)
    local env2 = tr and r.GetFXEnvelope(tr, 0, gp, false)
    local cnt = env2 and r.CountEnvelopePoints(env2) or -1
    local v1, v2 = -1, -1
    if cnt == 2 then
      local _, _, a = r.GetEnvelopePoint(env2, 0)
      local _, _, b = r.GetEnvelopePoint(env2, 1)
      v1, v2 = a, b
    end
    check("reopen: envelope kept", cnt == 2 and math.abs(v1 - 0.5) < 1e-3 and math.abs(v2 - 0.75) < 1e-3,
      string.format("points %d, values %.4f %.4f", cnt, v1, v2))
    check("render after reopen", tr ~= nil and render("env_reopen"))
  end

  -- editor window
  tr = r.GetTrack(0, 0)
  if tr then
    r.TrackFX_Show(tr, 0, 3)
    wait(1.5)
    local hw = r.TrackFX_GetFloatingWindow(tr, 0)
    check("open editor window", hw ~= nil, hw and "floating window open" or "no window")
    if hw ~= nil then
      local f = io.open(dir .. "/reaper_" .. fmt .. "_editor.open", "w")
      if f then f:write("open\n"); f:close() end
      wait(3.0) -- run.py takes a screenshot of the window now
    end
    r.TrackFX_Show(tr, 0, 2)
    wait(1.0)
    check("close editor window", r.TrackFX_GetFloatingWindow(tr, 0) == nil)
    r.TrackFX_Show(tr, 0, 3); wait(1.0); r.TrackFX_Show(tr, 0, 2); wait(0.5)
    check("editor open and close a second time", r.TrackFX_GetFloatingWindow(tr, 0) == nil)
  end
end

local co = coroutine.create(function()
  local ok, err = pcall(body)
  if not ok then check("script error", false, tostring(err)) end
  res.finished = true
  write_result()
  r.Main_OnCommand(40860, 0)
  r.Main_OnCommand(40004, 0) -- File: Quit REAPER; the project is saved, so no prompt
  done = true
end)

local function tick()
  if done then return end
  local ok, err = coroutine.resume(co)
  if not ok then
    res.fatal = tostring(err); write_result()
    r.Main_OnCommand(40004, 0)
    return
  end
  if coroutine.status(co) ~= "dead" then r.defer(tick) end
end
tick()
