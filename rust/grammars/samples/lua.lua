-- A tiny cart.
local Cart = {}
Cart.__index = Cart

function Cart.new()
  return setmetatable({ items = {} }, Cart)
end

function Cart:add(name, price)
  table.insert(self.items, { name = name, price = price })
end

function Cart:total()
  local sum = 0
  for _, item in ipairs(self.items) do sum = sum + item.price end
  return sum
end

return Cart
