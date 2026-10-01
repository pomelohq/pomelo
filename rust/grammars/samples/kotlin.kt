package shop

import kotlin.math.max

/** A line of an order. */
data class Line(val name: String, val price: Double, val qty: Int = 1)

fun total(lines: List<Line>): Double =
    lines.sumOf { it.price * it.qty }

fun main() {
    val lines = listOf(Line("tea", 3.0), Line("cake", 5.5, qty = 2))
    println("total: ${max(total(lines), 0.0)}")
}
