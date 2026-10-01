import gleam/io
import gleam/list

pub type Item {
  Item(name: String, price: Int)
}

/// The total of every item.
pub fn total(items: List(Item)) -> Int {
  list.fold(items, 0, fn(acc, item) { acc + item.price })
}

pub fn main() {
  io.println("ready")
}
