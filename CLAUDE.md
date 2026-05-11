# hprof-slurp

## Testing new features

When implementing or changing any feature, test against **both** sample heap dumps in the repo root:

- `JAVA_PROFILE_1.0.2.hprof` — JVM HPROF 1.0.2
- `JAVA_PROFILE_1.0.3.hprof` — Android HPROF 1.0.3 (`am dumpheap` extension tags)

Run the feature against each file and confirm output is correct on both before claiming the work is done. The two files exercise different format variants (notably the 1.0.3 Android extension tags handled by commit 504c6d0), so passing on one is not sufficient.

Typical invocation:

```
cargo run --release -- -i JAVA_PROFILE_1.0.2.hprof [feature flags]
cargo run --release -- -i JAVA_PROFILE_1.0.3.hprof [feature flags]
```

If a feature only applies to one format variant, state that explicitly and still run the other file to confirm no regression.
