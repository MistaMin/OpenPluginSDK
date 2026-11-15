-- OpenGrid9 (Ardour) headless worker for the SDK example ("Example Gain"). One phase per process.
-- Usage: opengrid9-lua og9_worker.lua <create|load> <session_dir> <session_name> <plugin_name> <type> <out_json> <pattern_file>
--   type: 0 Audio Unit, 2 LV2, 7 VST3 (Ardour's PluginType numbers)
-- This worker never deletes anything: run.py makes the session folder fresh before "create".
io.stdout:setvbuf("no")

local mode, sdir, sname, pname = arg[1], arg[2], arg[3], arg[4]
local ptype, outjson, patfile = tonumber(arg[5]), arg[6], arg[7]

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
    local n, isarr = 0, true
    for k in pairs(v) do n = n + 1; if type(k) ~= "number" then isarr = false end end
    if n == 0 then return "[]" end
    local p = {}
    if isarr then
      for i = 1, n do p[i] = jenc(v[i]) end
      return "[" .. table.concat(p, ",") .. "]"
    end
    for k, x in pairs(v) do p[#p + 1] = jenc(tostring(k)) .. ":" .. jenc(x) end
    return "{" .. table.concat(p, ",") .. "}"
  end
  return "null"
end

local out = { ok = false, step = "", err = "" }
local function wjson()
  local f = io.open(outjson, "w")
  if f then f:write(jenc(out)); f:close() end
end
local function fail(step, err)
  out.ok, out.step, out.err = false, step, tostring(err)
  wjson()
  print("FAIL " .. step .. ": " .. tostring(err))
  quit()
  error("stopped")
end

local ok, sess = pcall(function()
  if mode == "create" then return create_session(sdir, sname, 48000, 2, 2) end
  return load_session(sdir, sname)
end)
if not ok or not sess then fail("session", sess) end

local track
if mode == "create" then
  local rl
  ok, rl = pcall(function()
    return sess:new_audio_track(2, 2, ARDOUR.RouteGroup(), 1, "Audio", -1, ARDOUR.TrackMode.Normal, true)
  end)
  if not ok or not rl then fail("track", rl) end
  track = rl:front()
else
  ok, track = pcall(function() for r in sess:get_tracks():iter() do return r end end)
  if not ok or not track then fail("find-track", track) end
end

if mode == "create" then
  local pl
  pcall(function() ARDOUR.PluginManager.instance():refresh(false) end)
  ok, pl = pcall(function() return ARDOUR.LuaAPI.new_plugin(sess, pname, ptype, "") end)
  print("STEP new_plugin", tostring(ok), tostring(pl), ok and pl and tostring(pl:isnil()))
  if ok and pl and pl:isnil() then ok = false; pl = "no such plug-in in the host's plug-in list" end
  if not ok or not pl then fail("new_plugin", "plug-in '" .. pname .. "' of type " .. ptype .. " not found: " .. tostring(pl)) end
  print("STEP plug-in created")
  local t0 = os.clock()
  while os.clock() - t0 < tonumber(os.getenv("OG9_WAIT") or "0") do end
  local variant = os.getenv("OG9_ADD") or "index"
  if variant == "m1" then
    ok, pl = pcall(function() return track:add_processor_by_index(pl, -1, nil, true) end)
  elseif variant == "streams" then
    ok, pl = pcall(function() return track:add_processor_by_index(pl, 0, ARDOUR.ProcessorStreams(), true) end)
  elseif variant == "master" then
    local m = sess:master_out()
    print("STEP master", tostring(m))
    ok, pl = pcall(function() return m:add_processor_by_index(pl, 0, nil, true) end)
  elseif variant == "engine" then
    print("STEP engine running", tostring(AudioEngine:running()))
    ok, pl = pcall(function() return track:add_processor_by_index(pl, 0, nil, true) end)
  elseif variant == "noact" then
    ok, pl = pcall(function() return track:add_processor_by_index(pl, 0, nil, false) end)
  elseif variant == "list" then
    -- the list overload takes a different path through Route::add_processors
    ok, pl = pcall(function()
      local lst = ARDOUR.ProcessorList()
      lst:push_back(pl)
      return track:add_processors(lst, nil, nil)
    end)
  else
    ok, pl = pcall(function() return track:add_processor_by_index(pl, 0, nil, true) end)
  end
  if not ok then fail("add_processor", pl) end
  print("STEP plug-in added")
end

local ins
ok, ins = pcall(function()
  for i = 0, 16 do
    local p = track:nth_plugin(i)
    if not p then break end
    if p:name() then return p end
  end
end)
if not ok or not ins then fail("find_insert", ins) end
local pins = ins:to_plugininsert()
if not pins then fail("to_plugininsert", "nil") end
pcall(function() pins:activate() end)
local plg = pins:plugin(0)
if not plg then fail("plugin(0)", "nil") end

out.plugin_name = ins:name()
out.parameter_count = plg:parameter_count()
local labels, gi, wi = {}, nil, nil
for i = 0, out.parameter_count - 1 do
  local okl, lbl = pcall(function() return plg:parameter_label(i) end)
  lbl = (okl and lbl) and tostring(lbl) or "?"
  labels[i + 1] = lbl
  if lbl == "Gain" then gi = i end
  if lbl == "Warm" then wi = i end
end
out.labels, out.gain_index, out.warm_index = labels, gi, wi
if not (gi and wi) then fail("parameters", "Gain/Warm not found in: " .. table.concat(labels, "|")) end

-- value range of Gain as the host reports it
local lo, hi = 0.0, 1.0
pcall(function()
  local a, b = plg:get_parameter_descriptor(gi, ARDOUR.ParameterDescriptor())
  local pd = (type(b) == "table" and b[2]) or b
  out.descriptor_debug = tostring(a) .. "/" .. tostring(b)
  if pd and pd.upper > pd.lower then lo, hi = pd.lower, pd.upper end
end)
out.gain_range = { lo, hi }
local wlo, whi = 0.0, 1.0
pcall(function()
  local a, b = plg:get_parameter_descriptor(wi, ARDOUR.ParameterDescriptor()); local pd = (type(b) == "table" and b[2]) or b
  if pd and pd.upper > pd.lower then wlo, whi = pd.lower, pd.upper end
end)
out.warm_range = { wlo, whi }

-- LuaAPI.get/set_plugin_insert_param count control inputs only. For VST3 and AU that is the parameter
-- index; an LV2 plug-in lists its audio ports first, so map the port index to its control ordinal.
local function ordinal(i)
  local n = 0
  for k = 0, i - 1 do
    if plg:parameter_is_control(k) and plg:parameter_is_input(k) then n = n + 1 end
  end
  return n
end
local gc, wc = ordinal(gi), ordinal(wi)
out.control_ordinals = { gc, wc }
local function get(i) return ARDOUR.LuaAPI.get_plugin_insert_param(pins, i == gi and gc or wc) end
local function set(i, v) return ARDOUR.LuaAPI.set_plugin_insert_param(pins, i == gi and gc or wc, v) end

if mode == "create" then
  -- sweep Gain over its whole range, read each value back
  local worst, bad = 0, nil
  for s = 0, 8 do
    local v = lo + (hi - lo) * s / 8
    local oks = pcall(function() set(gi, v) end)
    local okg, rv = pcall(get, gi)
    if not oks or not okg or type(rv) ~= "number" then bad = "step " .. s; break end
    worst = math.max(worst, math.abs(rv - v) / (hi - lo))
  end
  out.sweep_ok = (bad == nil) and worst < 0.01
  out.sweep_detail = bad or string.format("9 steps over [%g, %g], worst read-back error %.3g of range", lo, hi, worst)

  -- Warm on and off
  set(wi, whi); local won = get(wi)
  set(wi, wlo); local woff = get(wi)
  local warm_ok = won > wlo + 0.5 * (whi - wlo) and woff < wlo + 0.5 * (whi - wlo)

  -- the values to keep: Gain at 11/16 of the range (+9 dB for -24..24), Warm on
  local gv = lo + (hi - lo) * 0.6875
  set(gi, gv); set(wi, whi)
  local gr, wr = get(gi), get(wi)
  out.set_ok = warm_ok and math.abs(gr - gv) < 0.01 * (hi - lo) and wr > wlo + 0.5 * (whi - wlo)
  out.set_detail = string.format("Gain set %.4f read %.4f; Warm off read %.2f, on read %.2f", gv, gr, woff, wr)
  -- let the engine run some cycles, so a VST3 processor sees the parameter changes before the state is saved
  pcall(function() ARDOUR.LuaAPI.usleep(1500000) end)
  out.persist = pcall(function() return sess:save_state("") end)
  pcall(function() close_session(sess) end)
  local f = io.open(patfile, "w")
  if f then f:write(string.format("%.9g %.9g\n", gr, wr)); f:close() end
  out.ok = true
  wjson()
  print("CREATE OK")
  quit()
else
  local f = io.open(patfile, "r")
  if not f then fail("pattern", "no pattern file") end
  local eg, ew = f:read("*n"), f:read("*n")
  f:close()
  local g, w = get(gi), get(wi)
  out.ok = math.abs(g - eg) < 0.01 * (hi - lo) and math.abs(w - ew) < 0.01 * (whi - wlo)
  out.detail = string.format("Gain %.4f -> %.4f, Warm %.2f -> %.2f", eg, g, ew, w)
  wjson()
  print(out.ok and "LOAD OK" or "LOAD MISMATCH")
  quit()
end
