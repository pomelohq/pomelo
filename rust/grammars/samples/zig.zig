const std = @import("std");

/// The total of the prices.
fn total(prices: []const f64) f64 {
    var sum: f64 = 0;
    for (prices) |price| sum += price;
    return sum;
}

pub fn main() !void {
    const prices = [_]f64{ 3.0, 5.5 };
    std.debug.print("total: {d}\n", .{total(&prices)});
}
