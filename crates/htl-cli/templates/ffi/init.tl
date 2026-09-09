local record {{mod}}
   record Greeting
      who: string
      text: string
   end
end

-- Reached from C through `{{mod}}_greet`, so this `error()` is what a caller sees as a
-- NULL return with the LUA status: the rule a mod breaks is enforced here, and no
-- wrapper in between has to know about it.
function {{mod}}.greet(who: string): {{mod}}.Greeting
   if who == "" then
      error("greet: `who` is empty")
   end
   return { who = who, text = "hello, " .. who }
end

return {{mod}}
