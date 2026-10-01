package shop

/** A line of an order. */
final case class Line(name: String, price: Double, qty: Int = 1)

object Totals {
  def total(lines: Seq[Line]): Double =
    lines.map(l => l.price * l.qty).sum

  def main(args: Array[String]): Unit = {
    val lines = List(Line("tea", 3.0), Line("cake", 5.5, 2))
    println(s"total: ${total(lines)}")
  }
}
