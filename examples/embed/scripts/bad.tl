local host = require("host")

-- type error on purpose: host:now() returns number, not string
local s: string = host:now()
print(s)
