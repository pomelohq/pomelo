import Foundation

/// A line of an order.
struct Line {
    let name: String
    let price: Double
    var qty: Int = 1
}

func total(_ lines: [Line]) -> Double {
    lines.reduce(0) { $0 + $1.price * Double($1.qty) }
}

let lines = [Line(name: "tea", price: 3), Line(name: "cake", price: 5.5, qty: 2)]
print("total: \(total(lines))")
