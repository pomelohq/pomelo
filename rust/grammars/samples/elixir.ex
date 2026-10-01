defmodule Cart do
  @moduledoc "Sums a cart."

  def total(items) when is_list(items) do
    items
    |> Enum.map(fn %{price: price, qty: qty} -> price * qty end)
    |> Enum.sum()
  end

  defp label(:ok), do: "ok"
  defp label(_), do: ~s(error)
end
