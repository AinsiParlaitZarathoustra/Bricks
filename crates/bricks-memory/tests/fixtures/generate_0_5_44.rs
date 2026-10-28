fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).expect("output path");
    let db = grafeo::GrafeoDB::open(path)?;
    db.execute("INSERT (:SchemaVersion {singleton: 'schema_version', version: 2})")?;
    db.execute("INSERT (:Memory {id: 'old_axum', content: 'Bricks utilise Axum et garde ses sources.', created_at: '2026-01-01T00:00:00Z', confidence: 0.75})")?;
    db.execute("INSERT (:Memory {id: 'old_rust', content: 'Bricks est écrit en Rust.', created_at: '2026-01-02T00:00:00Z', confidence: 0.5})")?;
    db.execute("MATCH (a:Memory {id: 'old_axum'}), (b:Memory {id: 'old_rust'}) INSERT (a)-[:RELATES_TO {relationship: 'implementation', weight: 0.8}]->(b)")?;
    db.create_text_index("Memory", "content")?;
    db.close()?;
    Ok(())
}
