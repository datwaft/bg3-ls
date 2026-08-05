local project = require("bg3.config").project({
  name = "MyMod",
  dependencies = {
    { name = "Item and Spell Bug Fixes", path = "../dependency" },
  },
})

return project
