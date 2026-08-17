local root = assert(vim.env.BG3_LS_ROOT)
local binary = root .. "/target/debug/bg3-ls"
local project = root .. "/test/fixtures/project"
local source = project .. "/Public/MyMod/Stats/Generated/Data/Passive.txt"
local lsx_source = project .. "/Public/MyMod/Progressions/Progressions.lsx"
local thoth_source = project .. "/Mods/MyMod/Scripts/thoth/helpers/MyMod.khn"
local osiris_source = project .. "/Mods/MyMod/Story/RawFiles/Goals/MainGoal.txt"
local cache = assert(vim.env.BG3_LS_TEST_CACHE)
local dependency = vim.fn.tempname()
local dependency_file = dependency .. "/Public/Fixes/Stats/Generated/Data/Passive.txt"
local dependency_helper = dependency .. "/Mods/Fixes/Scripts/thoth/helpers/Watched.khn"
local dependency_fixture_helper = dependency .. "/Mods/Fixes/Scripts/thoth/helpers/Fixes.khn"
local dependency_goal = dependency .. "/Mods/Fixes/Story/RawFiles/Goals/WatchedGoal.txt"
local dependency_fixture_goal = dependency .. "/Mods/Fixes/Story/RawFiles/Goals/FixesGoal.txt"
local localization_source = dependency .. "/Mods/Fixes/Localization/English/hover.xml"
vim.fn.mkdir(vim.fs.dirname(dependency_file), "p")
vim.fn.writefile(
  vim.fn.readfile(root .. "/test/fixtures/dependency/Public/Fixes/Stats/Generated/Data/Passive.txt"),
  dependency_file
)
vim.fn.mkdir(vim.fs.dirname(dependency_fixture_helper), "p")
vim.fn.writefile(
  vim.fn.readfile(root .. "/test/fixtures/dependency/Mods/Fixes/Scripts/thoth/helpers/Fixes.khn"),
  dependency_fixture_helper
)
vim.fn.mkdir(vim.fs.dirname(dependency_fixture_goal), "p")
vim.fn.writefile(
  vim.fn.readfile(root .. "/test/fixtures/dependency/Mods/Fixes/Story/RawFiles/Goals/FixesGoal.txt"),
  dependency_fixture_goal
)
vim.fn.mkdir(vim.fs.dirname(localization_source), "p")
vim.fn.writefile({
  '<contentList><content contentuid="h333333333333333333333333333333333333" version="1">&lt;LSTag Type="Passive" Tooltip="CONSUMER">consumer&lt;/LSTag&gt;</content></contentList>',
}, localization_source)

local progress = {}
local completed_progress = 0
local autocmd = vim.api.nvim_create_autocmd("LspProgress", {
  callback = function(event)
    progress[#progress + 1] = event.data.params.value.kind
    if event.data.params.value.kind == "end" then
      completed_progress = completed_progress + 1
    end
  end,
})

vim.cmd("edit " .. vim.fn.fnameescape(source))
vim.bo.filetype = "bg3_stats"
local lsp_config = {
  name = "bg3",
  cmd = { binary, "--cache-dir", cache },
  root_dir = project,
  init_options = {
    game_data = root .. "/test/fixtures/game",
    base_modules = { "Shared" },
    project = {
      name = "MyMod",
      dependencies = {
        {
          name = "Item and Spell Bug Fixes",
          path = dependency,
        },
      },
    },
  },
}
local client_id = assert(vim.lsp.start(lsp_config))
local client = assert(vim.lsp.get_client_by_id(client_id))
assert(vim.wait(5000, function()
  return client.initialized and vim.lsp.buf_is_attached(0, client_id)
end, 10), "the Neovim client did not initialize")

local early_definition
vim.lsp.buf_request(0, "textDocument/definition", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = { line = 6, character = 8 },
}, function(error, result)
  assert(not error, vim.inspect(error))
  early_definition = result
end)

assert(vim.wait(10000, function()
  local responses = vim.lsp.buf_request_sync(0, "workspace/executeCommand", {
    command = "bg3.indexInfo",
    arguments = {},
  }, 1000)
  local response = responses and responses[client_id]
  return response and response.result and response.result.generation == 1
end, 50), "the Rust BG3 index did not become ready")

assert(client.server_capabilities.completionProvider)
assert(client.server_capabilities.signatureHelpProvider)

assert(vim.wait(5000, function()
  return early_definition ~= nil
end, 50), "the definition request queued during initial indexing did not finish")
assert(#early_definition == 3, vim.inspect(early_definition))

local tooltip_source = vim.api.nvim_buf_get_lines(0, 4, 5, false)[1]
local tooltip_start = assert(tooltip_source:find("CONSUMER", 1, true)) - 1
local hover_response = vim.lsp.buf_request_sync(0, "textDocument/hover", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = { line = 4, character = tooltip_start },
}, 5000)
local hover = assert(hover_response[client_id] and hover_response[client_id].result)
local hover_text = hover.contents.value
assert(hover_text:find("---", 1, true), hover_text)
assert(hover_text:find("Test action & label", 1, true), hover_text)
assert(hover_text:find("Synthetic description", 1, true), hover_text)
assert(hover_text:find("aaaaaaaa%-aaaa%-aaaa%-aaaa%-aaaaaaaaaaaa"), hover_text)

vim.cmd("edit " .. vim.fn.fnameescape(localization_source))
vim.bo.filetype = "bg3_localization"
assert(vim.lsp.buf_attach_client(0, client_id))
local localization_line = vim.api.nvim_buf_get_lines(0, 0, 1, false)[1]
local localization_tooltip = assert(localization_line:find("CONSUMER", 1, true)) - 1
local localization_hover_response = vim.lsp.buf_request_sync(0, "textDocument/hover", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = { line = 0, character = localization_tooltip },
}, 5000)
local localization_hover = assert(
  localization_hover_response[client_id] and localization_hover_response[client_id].result
)
local localization_hover_text = localization_hover.contents.value
assert(localization_hover_text:find("PassiveData", 1, true), localization_hover_text)
assert(localization_hover_text:find("Test action & label", 1, true), localization_hover_text)
assert(localization_hover_text:find("Synthetic description", 1, true), localization_hover_text)

vim.cmd("edit " .. vim.fn.fnameescape(source))

local function replace_buffer(lines)
  vim.api.nvim_buf_set_lines(0, 0, -1, false, lines)
end

local completion_line = 'data "Boosts" "UnlockSpell(Target_T'
replace_buffer({
  'new entry "TEST"',
  'type "PassiveData"',
  completion_line,
})
assert(vim.wait(5000, function()
  local results = vim.lsp.buf_request_sync(0, "textDocument/completion", {
    textDocument = { uri = vim.uri_from_bufnr(0) },
    position = { line = 2, character = #completion_line },
  }, 1000)
  local result = results and results[client_id] and results[client_id].result
  local items = result and (result.items or result)
  return items and vim.iter(items):any(function(item)
    return item.label == "Target_Test"
  end)
end, 50), "unsaved typed-symbol completion did not include Target_Test")

local signature_line = 'data "Boosts" "ApplyStatus(TEST_STATUS,'
replace_buffer({
  'new entry "TEST"',
  'type "PassiveData"',
  signature_line,
})
assert(vim.wait(5000, function()
  local results = vim.lsp.buf_request_sync(0, "textDocument/signatureHelp", {
    textDocument = { uri = vim.uri_from_bufnr(0) },
    position = { line = 2, character = #signature_line },
  }, 1000)
  local result = results and results[client_id] and results[client_id].result
  return result and result.activeParameter == 1
end, 50), "signature help did not select the second parameter")

replace_buffer({
  'new entry "TEST"',
  'type "PassiveData"',
  'data "Enabled" "Maybe"',
})
assert(vim.wait(5000, function()
  return vim.iter(vim.diagnostic.get(0)):any(function(diagnostic)
    return diagnostic.code == "invalid-enum"
  end)
end, 50), "the server did not publish the invalid-enum diagnostic")

assert(vim.wait(5000, function()
  return completed_progress >= 1
end, 50), "the initial index progress did not finish")

if vim.env.BG3_LS_SKIP_WATCHER_TESTS ~= "1" then
  -- The coordinator installs the watcher immediately after it closes initial
  -- progress. Give the OS watcher registration one event-loop turn to complete.
  vim.wait(250)

  local function wait_for_refresh(previous_generation, message)
    assert(vim.wait(10000, function()
      return completed_progress > previous_generation
    end, 50), message)
  end

  local previous_generation = completed_progress
  vim.fn.writefile({
    'new entry "CHAINED"',
    'type "PassiveData"',
    'data "Boosts" "UnlockSpell(Target_Test)"',
    '',
    'new entry "WATCHED"',
    'type "PassiveData"',
  }, dependency_file)
  wait_for_refresh(previous_generation, "the filesystem watcher did not publish a new index generation")

  local watched = vim.lsp.buf_request_sync(0, "workspace/symbol", { query = "WATCHED" }, 5000)
  assert(watched[client_id] and #watched[client_id].result == 1, vim.inspect(watched))

  previous_generation = completed_progress
  vim.fn.mkdir(vim.fs.dirname(dependency_helper), "p")
  vim.fn.writefile({
    "function WatchedHelper(value)",
    "  return value",
    "end",
  }, dependency_helper)
  wait_for_refresh(previous_generation, "the watcher did not index an added Thoth helper")
  local added = vim.lsp.buf_request_sync(0, "workspace/symbol", { query = "WatchedHelper" }, 5000)
  assert(added[client_id] and #added[client_id].result == 1, vim.inspect(added))

  previous_generation = completed_progress
  vim.fn.writefile({
    "function ChangedHelper(value, fallback)",
    "  return value or fallback",
    "end",
  }, dependency_helper)
  wait_for_refresh(previous_generation, "the watcher did not update a changed Thoth helper")
  local changed = vim.lsp.buf_request_sync(0, "workspace/symbol", { query = "ChangedHelper" }, 5000)
  assert(changed[client_id] and #changed[client_id].result == 1, vim.inspect(changed))

  previous_generation = completed_progress
  vim.fn.delete(dependency_helper)
  wait_for_refresh(previous_generation, "the watcher did not remove a deleted Thoth helper")
  local removed = vim.lsp.buf_request_sync(0, "workspace/symbol", { query = "ChangedHelper" }, 5000)
  assert(removed[client_id] and #removed[client_id].result == 0, vim.inspect(removed))

  previous_generation = completed_progress
  vim.fn.mkdir(vim.fs.dirname(dependency_goal), "p")
  vim.fn.writefile({
    "Version 1",
    "SubGoalCombiner SGC_AND",
    "INITSECTION",
    "KBSECTION",
    "PROC",
    "WatchedProc((INTEGER)_Value)",
    "THEN",
    "DB_Watched(_Value);",
    "EXITSECTION",
    "ENDEXITSECTION",
  }, dependency_goal)
  wait_for_refresh(previous_generation, "the watcher did not index an added Osiris goal")
  local added_goal = vim.lsp.buf_request_sync(0, "workspace/symbol", { query = "WatchedProc" }, 5000)
  assert(added_goal[client_id] and #added_goal[client_id].result == 1, vim.inspect(added_goal))

  previous_generation = completed_progress
  vim.fn.writefile({
    "Version 1",
    "SubGoalCombiner SGC_AND",
    "INITSECTION",
    "KBSECTION",
    "PROC",
    "ChangedProc((INTEGER)_Value)",
    "THEN",
    "DB_Watched(_Value);",
    "EXITSECTION",
    "ENDEXITSECTION",
  }, dependency_goal)
  wait_for_refresh(previous_generation, "the watcher did not update a changed Osiris goal")
  local changed_goal = vim.lsp.buf_request_sync(0, "workspace/symbol", { query = "ChangedProc" }, 5000)
  assert(changed_goal[client_id] and #changed_goal[client_id].result == 1, vim.inspect(changed_goal))

  previous_generation = completed_progress
  vim.fn.delete(dependency_goal)
  wait_for_refresh(previous_generation, "the watcher did not remove a deleted Osiris goal")
  local removed_goal = vim.lsp.buf_request_sync(0, "workspace/symbol", { query = "ChangedProc" }, 5000)
  assert(removed_goal[client_id] and #removed_goal[client_id].result == 0, vim.inspect(removed_goal))
end

vim.lsp.buf_detach_client(0, client_id)
assert(vim.wait(5000, function()
  return vim.tbl_isempty(vim.diagnostic.get(0))
end, 50), "diagnostics were not cleared after didClose")

vim.bo.modified = false
vim.cmd("edit " .. vim.fn.fnameescape(dependency_file))
vim.bo.filetype = "bg3_stats"
local reused_client_id = assert(vim.lsp.start(lsp_config))
assert(reused_client_id == client_id, "the dependency buffer did not reuse the project client")
local dependency_definition = vim.lsp.buf_request_sync(0, "textDocument/definition", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = { line = 2, character = 30 },
}, 5000)
assert(
  dependency_definition[client_id] and #dependency_definition[client_id].result >= 1,
  vim.inspect(dependency_definition)
)

vim.bo.modified = false
vim.cmd("edit " .. vim.fn.fnameescape(lsx_source))
vim.bo.filetype = "bg3_lsx"
local lsx_client_id = assert(vim.lsp.start(lsp_config))
assert(lsx_client_id == client_id, "the LSX buffer did not reuse the project client")
assert(vim.wait(5000, function()
  return vim.lsp.buf_is_attached(0, client_id)
end, 10), "the project client did not attach to the LSX buffer")

local lsx_completion_line = '<attribute id="Boosts" type="LSString" value="ActionResource(ActionPoint,1)"/>'
local lsx_completion_column = assert(lsx_completion_line:find("ActionP", 1, true)) - 1 + #"ActionP"
local lsx_passive_line = '<attribute id="PassivesAdded" type="LSString" value="CHAINED"/>'
replace_buffer({
  '<node id="Progression">',
  lsx_completion_line,
  '<attribute id="Name" type="LSString" value="UnsavedProgression"/>',
  lsx_passive_line,
  '<attribute id="UUID" type="guid" value="99999999-9999-9999-9999-999999999999"/>',
  "</node>",
})
assert(vim.wait(5000, function()
  local results = vim.lsp.buf_request_sync(0, "textDocument/completion", {
    textDocument = { uri = vim.uri_from_bufnr(0) },
    position = { line = 1, character = lsx_completion_column },
  }, 1000)
  local result = results and results[client_id] and results[client_id].result
  local items = result and (result.items or result)
  return items and vim.iter(items):any(function(item)
    return item.label == "ActionPoint"
  end)
end, 50), "LSX completion did not include ActionPoint")

local lsx_signature = vim.lsp.buf_request_sync(0, "textDocument/signatureHelp", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = { line = 1, character = lsx_completion_column },
}, 5000)
assert(lsx_signature[client_id] and lsx_signature[client_id].result, vim.inspect(lsx_signature))

local lsx_passive_column = assert(lsx_passive_line:find("CHAINED", 1, true)) - 1
local lsx_definition = vim.lsp.buf_request_sync(0, "textDocument/definition", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = { line = 3, character = lsx_passive_column },
}, 5000)
assert(lsx_definition[client_id] and #lsx_definition[client_id].result == 3, vim.inspect(lsx_definition))
assert(vim.wait(1000, function()
  return vim.tbl_isempty(vim.diagnostic.get(0))
end, 50), "the server published legacy Stats diagnostics for LSX")

vim.bo.modified = false
vim.cmd("edit " .. vim.fn.fnameescape(thoth_source))
vim.bo.filetype = "bg3_thoth"
local thoth_client_id = assert(vim.lsp.start(lsp_config))
assert(thoth_client_id == client_id, "the Thoth buffer did not reuse the project client")
assert(vim.wait(5000, function()
  return vim.lsp.buf_is_attached(0, client_id)
end, 10), "the project client did not attach to the Thoth buffer")

replace_buffer({
  "function UnsavedCaller(value)",
  "  return DependencyOnly(value)",
  "end",
})
assert(vim.wait(5000, function()
  local result = vim.lsp.buf_request_sync(0, "textDocument/documentSymbol", {
    textDocument = { uri = vim.uri_from_bufnr(0) },
  }, 1000)
  local symbols = result and result[client_id] and result[client_id].result
  return symbols and #symbols == 1 and symbols[1].name == "UnsavedCaller"
end, 50), "the unsaved Thoth overlay did not replace the disk declarations")

local thoth_definition = vim.lsp.buf_request_sync(0, "textDocument/definition", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = { line = 1, character = 10 },
}, 5000)
assert(thoth_definition[client_id] and #thoth_definition[client_id].result == 1, vim.inspect(thoth_definition))

local thoth_signature = vim.lsp.buf_request_sync(0, "textDocument/signatureHelp", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = { line = 1, character = #"  return DependencyOnly(" },
}, 5000)
assert(thoth_signature[client_id] and thoth_signature[client_id].result, vim.inspect(thoth_signature))
assert(
  thoth_signature[client_id].result.signatures[1].label == "DependencyOnly(value)",
  vim.inspect(thoth_signature)
)

assert(vim.wait(1000, function()
  return vim.tbl_isempty(vim.diagnostic.get(0))
end, 50), "the server published diagnostics for Thoth source")

replace_buffer({
  "function Broken(",
  "  return value",
  "end",
})
assert(vim.wait(5000, function()
  return vim.iter(vim.diagnostic.get(0)):any(function(diagnostic)
    return diagnostic.code == "thoth-syntax-error" and diagnostic.source == "bg3"
  end)
end, 50), "the server did not publish the Thoth syntax diagnostic")

replace_buffer({
  "function UnsavedCaller(value)",
  "  return DependencyOnly(value)",
  "end",
})
assert(vim.wait(5000, function()
  return vim.tbl_isempty(vim.diagnostic.get(0))
end, 50), "the Thoth syntax diagnostic remained after restoring valid source")

vim.bo.modified = false
vim.cmd("edit " .. vim.fn.fnameescape(osiris_source))
vim.bo.filetype = "bg3_osiris"
local osiris_client_id = assert(vim.lsp.start(lsp_config))
assert(osiris_client_id == client_id, "the Osiris buffer did not reuse the project client")
assert(vim.wait(5000, function()
  return vim.lsp.buf_is_attached(0, client_id)
end, 10), "the project client did not attach to the Osiris buffer")

local function osiris_position(needle)
  for line, source_line in ipairs(vim.api.nvim_buf_get_lines(0, 0, -1, false)) do
    local column = source_line:find(needle, 1, true)
    if column then
      return { line = line - 1, character = column - 1 }
    end
  end
  error("missing Osiris test token: " .. needle)
end

local database_definition = vim.lsp.buf_request_sync(0, "textDocument/definition", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = osiris_position("DB_Tracked"),
}, 5000)
assert(
  database_definition[client_id] and #database_definition[client_id].result == 4,
  vim.inspect(database_definition)
)

local database_references = vim.lsp.buf_request_sync(0, "textDocument/references", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = osiris_position("DB_Tracked"),
  context = { includeDeclaration = true },
}, 5000)
assert(
  database_references[client_id] and #database_references[client_id].result == 10,
  vim.inspect(database_references)
)

local parent_definition = vim.lsp.buf_request_sync(0, "textDocument/definition", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = osiris_position("SharedGoal"),
}, 5000)
assert(parent_definition[client_id] and #parent_definition[client_id].result == 1, vim.inspect(parent_definition))

local database_hover = vim.lsp.buf_request_sync(0, "textDocument/hover", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = osiris_position("DB_Tracked"),
}, 5000)
local database_hover_text = assert(database_hover[client_id] and database_hover[client_id].result).contents.value
assert(database_hover_text:find("DB_Tracked/2", 1, true), database_hover_text)
assert(database_hover_text:find("DB_Tracked(CHARACTER, INTEGER)", 1, true), database_hover_text)

local osiris_symbols = vim.lsp.buf_request_sync(0, "textDocument/documentSymbol", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
}, 5000)
assert(osiris_symbols[client_id] and #osiris_symbols[client_id].result == 5, vim.inspect(osiris_symbols))

local signature_position = osiris_position("DB_Tracked(_Actor, _Count)")
signature_position.character = signature_position.character + #"DB_Tracked(_Actor,"
local osiris_signature = vim.lsp.buf_request_sync(0, "textDocument/signatureHelp", {
  textDocument = { uri = vim.uri_from_bufnr(0) },
  position = signature_position,
}, 5000)
assert(osiris_signature[client_id] and osiris_signature[client_id].result, vim.inspect(osiris_signature))
assert(
  osiris_signature[client_id].result.signatures[1].label == "DB_Tracked(CHARACTER, INTEGER)",
  vim.inspect(osiris_signature)
)
assert(osiris_signature[client_id].result.activeParameter == 1, vim.inspect(osiris_signature))

replace_buffer({
  "Version 1",
  "SubGoalCombiner SGC_AND",
  "INITSECTION",
  "KBSECTION",
  "IF",
  "Event()",
  "THEN",
  "DB_Tr",
  "EXITSECTION",
  "ENDEXITSECTION",
})
assert(vim.wait(5000, function()
  local results = vim.lsp.buf_request_sync(0, "textDocument/completion", {
    textDocument = { uri = vim.uri_from_bufnr(0) },
    position = { line = 7, character = 5 },
  }, 1000)
  local result = results and results[client_id] and results[client_id].result
  local items = result and (result.items or result)
  return items and vim.iter(items):any(function(item)
    return item.label == "DB_Tracked"
  end)
end, 50), "Osiris completion did not include DB_Tracked")

replace_buffer({
  "Version 1",
  "SubGoalCombiner SGC_AND",
  "INITSECTION",
  "KBSECTION",
  "PROC",
  "UnsavedProc((INTEGER)_Value)",
  "THEN",
  "DB_Unsaved(_Value);",
  "EXITSECTION",
  "ENDEXITSECTION",
})
assert(vim.wait(5000, function()
  local result = vim.lsp.buf_request_sync(0, "textDocument/documentSymbol", {
    textDocument = { uri = vim.uri_from_bufnr(0) },
  }, 1000)
  local symbols = result and result[client_id] and result[client_id].result
  return symbols
    and vim.iter(symbols):any(function(symbol)
      return symbol.name == "UnsavedProc"
    end)
    and vim.iter(symbols):any(function(symbol)
      return symbol.name == "DB_Unsaved"
    end)
end, 50), "the unsaved Osiris overlay did not replace disk declarations")

replace_buffer({
  "Version 1",
  "SubGoalCombiner SGC_AND",
  "INITSECTION",
  "KBSECTION",
  "IF",
  "Broken(",
  "EXITSECTION",
  "ENDEXITSECTION",
})
assert(vim.wait(5000, function()
  return vim.iter(vim.diagnostic.get(0)):any(function(diagnostic)
    return diagnostic.code == "osiris-syntax-error" and diagnostic.source == "bg3"
  end)
end, 50), "the server did not publish the Osiris syntax diagnostic")

replace_buffer(vim.fn.readfile(osiris_source))
assert(vim.wait(5000, function()
  return vim.tbl_isempty(vim.diagnostic.get(0))
end, 50), "the Osiris syntax diagnostic remained after restoring valid source")

assert(vim.tbl_contains(progress, "begin"), vim.inspect(progress))
assert(vim.tbl_contains(progress, "end"), vim.inspect(progress))

vim.api.nvim_del_autocmd(autocmd)
client:stop(true)
vim.fn.delete(dependency, "rf")
vim.cmd("qa!")
