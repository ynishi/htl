-- On purpose: a type error. require("broken") fails instead of falling through.
local n: number = "not a number"
return n
