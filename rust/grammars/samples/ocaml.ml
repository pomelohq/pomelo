(* Sum the prices of an order. *)
type line = { name : string; price : float; qty : int }

let total lines =
  List.fold_left (fun acc l -> acc +. (l.price *. float_of_int l.qty)) 0.0 lines

let () =
  let lines = [ { name = "tea"; price = 3.0; qty = 2 } ] in
  Printf.printf "total: %.2f\n" (total lines)
