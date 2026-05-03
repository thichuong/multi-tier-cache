# Publishing to crates.io

## Prerequisites

Ensure you have an account on [crates.io](https://crates.io) and have generated an API token.

## Steps

1.  **Login** (only needed once):
    ```bash
    cargo login <your-api-token>
    ```

2.  **Verify Package** (Dry Run):
    Check that the package builds and contains the correct files without actually publishing.
    ```bash
    cargo publish --dry-run
    ```

3.  **Publish**:
    Upload the package to crates.io.
    ```bash
    cargo publish
    ```
