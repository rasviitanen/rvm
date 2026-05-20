# RVM Tour game service

This is a small HTTP control-plane service for the QUIC game demo. It is meant
to show the non-simulation pieces of a Zwift-like game running as an RVM guest:
login, profile, race discovery, matchmaking, and session bootstrap.

Build it as a WASI component:

```sh
cargo build -p game-service --target wasm32-wasip2
```

Deploy it to a running RVM host:

```sh
curl --data-binary "@target/wasm32-wasip2/debug/game_service.wasm" localhost:8002/deploy/game-service
```

Try the flow:

```sh
curl -X POST "http://127.0.0.1:8000/game-service/login?rider=Ada"
curl -H "Authorization: Bearer rvmtour-demo-token" http://127.0.0.1:8000/game-service/races
curl -X POST -H "Authorization: Bearer rvmtour-demo-token" "http://127.0.0.1:8000/game-service/matchmaking/join?race=crit-city-1830"
curl -H "Authorization: Bearer rvmtour-demo-token" http://127.0.0.1:8000/game-service/session/bootstrap
curl -H "Authorization: Bearer rvmtour-demo-token" http://127.0.0.1:8000/game-service/history/workouts
curl -H "Authorization: Bearer rvmtour-demo-token" http://127.0.0.1:8000/game-service/history/races
curl -H "Authorization: Bearer rvmtour-demo-token" http://127.0.0.1:8000/game-service/history/stats
```

After a game client disconnects from the QUIC gameserver, the gameserver stores a
full workout document and mirrors it into the history KV. Fetch a specific item
with:

```sh
curl -H "Authorization: Bearer rvmtour-demo-token" http://127.0.0.1:8000/game-service/history/workouts/workout-client-0-samples-120
curl -H "Authorization: Bearer rvmtour-demo-token" http://127.0.0.1:8000/game-service/history/races/crit-city-1830
```
