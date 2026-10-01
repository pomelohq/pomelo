module Main exposing (main)

import Html exposing (text)


type Msg
    = Increment
    | Decrement


update : Msg -> Int -> Int
update msg count =
    case msg of
        Increment ->
            count + 1

        Decrement ->
            count - 1


main =
    text (String.fromInt (update Increment 41))
