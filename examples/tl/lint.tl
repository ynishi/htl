-- htl lint samples: Teal accepts this file, htl flags two hazards.

local enum Kind
   "add"
   "remove"
   "clear"
end

local record Msg
   kind: Kind
   n: number
end

local by_id: {string:Msg} = {}

-- nil-index: by_id["x"] is typed Msg, but the lookup can miss at runtime.
local n = by_id["x"].n
print(n)

-- enum-exhaustive: "clear" is not handled and there is no else.
local function apply(m: Msg)
   if m.kind == "add" then
      print("add", m.n)
   elseif m.kind == "remove" then
      print("remove", m.n)
   end
end

-- Same shapes, silenced on purpose.
local m2 = by_id["y"].n -- htl: allow(nil-index)
print(m2)

local function apply2(m: Msg)
   if m.kind == "add" or m.kind == "remove" then
      print("change")
   else
      print("clear")
   end
end

apply({ kind = "add", n = 1 })
apply2({ kind = "clear", n = 0 })
