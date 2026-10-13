"""Create a fresh, non-secret SQLx compile-time schema and print its URL."""
import sqlite3
import tempfile
from pathlib import Path

with tempfile.NamedTemporaryFile(prefix="aime-sqlx-", suffix=".sqlite", delete=False) as fixture:
    path = fixture.name
with sqlite3.connect(path) as db:
    migrations = Path(__file__).resolve().parent.parent / "src-tauri" / "migrations"
    for migration in sorted(migrations.glob("*.sql"), key=lambda p: int(p.name.split("_")[0])):
        db.executescript(migration.read_text())
print(f"sqlite:{path}")
