import { readdir, rename } from 'node:fs/promises';
import { join } from 'node:path';

// Cargo discovers every literal Cargo.toml in Git dependencies, including scaffolds.
// Materialize template manifests only in the generated standalone project.
export async function materializeManifests(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) await materializeManifests(path);
    else if (entry.isFile() && entry.name === 'Cargo.toml.template')
      await rename(path, join(directory, 'Cargo.toml'));
  }
}
