# Sum prices by name.
library(stats)

total <- function(lines) {
  sum(lines$price * lines$qty, na.rm = TRUE)
}

lines <- data.frame(name = c("tea", "cake"), price = c(3, 5.5), qty = c(2L, 1L))
if (total(lines) > 10) print("large") else print(paste("total:", total(lines)))
