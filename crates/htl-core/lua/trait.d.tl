-- Type declarations for htl.trait (runtime: trait.lua).
--
-- Default methods for a record that implements an interface. A Teal `interface` holds
-- signatures only; the bodies of its default methods go in a table typed with the
-- interface, and `impl` puts them on each record:
--
--    local trait = require("htl.trait")
--
--    local interface Shape
--       area: function(self): number
--       describe: function(self): string
--    end
--
--    local Shape_default: Shape = {
--       describe = function(self: Shape): string
--          return ("area=%.2f"):format(self:area())
--       end,
--    }
--
--    local record Circle is Shape
--       r: number
--    end
--    function Circle:area(): number return math.pi * self.r ^ 2 end
--    trait.impl(Shape_default, Circle)
local record trait
   --- Copy into `target` every key of `defaults` that `target` does not already hold.
   ---
   --- "Holds" is `rawget(target, key) ~= nil`: a key the record defines itself is kept,
   --- a field set to `false` included, and nothing is overwritten. The copy is raw
   --- (`next` over `defaults`, `rawget` / `rawset` on `target`): no metatable of either
   --- table is read or written, so a `__index` or `__newindex` on the record has no say,
   --- and the default ends up as one of `target`'s own fields, beside the methods the
   --- record defines. Returns nothing; it runs once, where the record is declared.
   ---
   --- The signature is `function<I>(defaults: I, target: I)`, in this order, because it
   --- is the form that checks without a cast and refuses a `target` that is not `is I`:
   --- `trait.impl(Shape_default, Other)` for a record `Other` that does not implement
   --- `Shape` is `argument 2: got Other, expected interface Shape`. With `target` first,
   --- `I` is inferred from the record and the defaults table is what fails to match.
   impl: function<I>(defaults: I, target: I)
end

return trait
