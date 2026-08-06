vim.opt.runtimepath:prepend(vim.env.BG3_LS_TREE_SITTER)
vim.filetype.add({ extension = { khn = "bg3_thoth", lsx = "bg3_lsx", txt = "bg3_stats" } })
vim.treesitter.language.register("xml", "bg3_lsx")
