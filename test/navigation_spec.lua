local root = assert(vim.env.BG3_LS_ROOT)
local binary = root .. "/target/debug/bg3-ls"
local project = root .. "/test/fixtures/project"
local source = project .. "/Public/MyMod/Stats/Generated/Data/Passive.txt"
local lsx_source = project .. "/Public/MyMod/Progressions/Progressions.lsx"
local cache = assert(vim.env.BG3_LS_TEST_CACHE)
local dependency = vim.fn.tempname()
local dependency_file = dependency .. "/Public/Fixes/Stats/Generated/Data/Passive.txt"
vim.fn.mkdir(vim.fs.dirname(dependency_file), "p")
vim.fn.writefile(
  vim.fn.readfile(root .. "/test/fixtures/dependency/Public/Fixes/Stats/Generated/Data/Passive.txt"),
  dependency_file
)

local progress = {}
local autocmd = vim.api.nvim_create_autocmd("LspProgress", {
  callback = function(event)
    progress[#progress + 1] = event.data.params.value.kind
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

vim.fn.writefile({
  'new entry "CHAINED"',
  'type "PassiveData"',
  'data "Boosts" "UnlockSpell(Target_Test)"',
  '',
  'new entry "WATCHED"',
  'type "PassiveData"',
}, dependency_file)
assert(vim.wait(10000, function()
  local responses = vim.lsp.buf_request_sync(0, "workspace/executeCommand", {
    command = "bg3.indexInfo",
    arguments = {},
  }, 1000)
  local response = responses and responses[client_id]
  return response
    and response.result
    and response.result.generation
    and response.result.generation >= 2
end, 50), "the filesystem watcher did not publish a new index generation")

local watched = vim.lsp.buf_request_sync(0, "workspace/symbol", { query = "WATCHED" }, 5000)
assert(watched[client_id] and #watched[client_id].result == 1, vim.inspect(watched))

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

assert(vim.tbl_contains(progress, "begin"), vim.inspect(progress))
assert(vim.tbl_contains(progress, "end"), vim.inspect(progress))

vim.api.nvim_del_autocmd(autocmd)
client:stop(true)
vim.fn.delete(dependency, "rf")
vim.cmd("qa!")
