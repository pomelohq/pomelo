using System;

namespace Shop
{
    // An order line with a price.
    public record Line(string Name, decimal Price);

    public static class Totals
    {
        public static decimal Sum(Line[] lines)
        {
            var total = 0m;
            foreach (var line in lines) { total += line.Price; }
            return total > 100 ? total * 0.9m : total;
        }
    }
}
