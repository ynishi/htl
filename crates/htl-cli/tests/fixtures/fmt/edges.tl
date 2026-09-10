

-- htl fmt sample: the whitespace edges. Blank lines lead and trail this file, one run in
-- the middle is longer than two, three lines end in spaces, and the last line has no
-- newline after it. Every one of those is something `htl fmt` normalises, so leave them
-- exactly as they are.
--[[
      a long comment, whose interior is left where it is
   local not_code = 1
]]
local record Handler
  run: function(string): boolean   
end




local function each(items: {string}, f: function(string))
for _, it in ipairs(items) do
f(it)   
end
end

local h: Handler = {
run = function(s: string): boolean
return #s >
0
end,
}

each({ "a", "b" }, function(s: string)
print(s,
h.run(s))
end)   
   