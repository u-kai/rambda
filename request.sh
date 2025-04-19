seq 10 | xargs -n1 -P10 -I{} curl localhost:9001 --json '{"key": "value"}' -v
