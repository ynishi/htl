-- htl.trait: default methods for a record that implements an interface.
--
-- Teal's `interface` is a type and nothing else: it cannot hold a function body, so a
-- default method has to live in a table beside it (`local Shape_default: Shape = {...}`)
-- and be put on each record that wants it. `impl` is that one step.
--
--    trait.impl(Shape_default, Circle)
--
-- What `impl` promises, and its types, are `trait.d.tl`'s; why the defaults are copied
-- rather than chained through `__index` is `trait_lib.rs`'s module doc.
local trait = {}

function trait.impl(defaults, target)
   for key, value in next, defaults do
      if rawget(target, key) == nil then
         rawset(target, key, value)
      end
   end
end

return trait
