module Main where

import Data.List (foldl')

-- | Sum of the prices.
total :: [(String, Int)] -> Int
total = foldl' (\acc (_, price) -> acc + price) 0

data Status = Paid | Unpaid deriving (Show, Eq)

main :: IO ()
main = do
  let items = [("tea", 3), ("cake", 5)]
  print (total items, Paid)
