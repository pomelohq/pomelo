-module(cart).
-export([total/1]).

%% Sums prices times quantities.
total(Items) ->
    lists:foldl(fun({_Name, Price, Qty}, Acc) -> Acc + Price * Qty end, 0, Items).

label(ok) -> <<"ok">>;
label(_) -> "error".
