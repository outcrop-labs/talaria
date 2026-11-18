use crate::db::{Db, Error, Note, User};

/// One note by id, for its owner.
pub async fn note_by_id(db: &Db, user: &User, id: &str) -> Result<Note, Error> {
    db.query_one("select * from notes where id = $1 and owner = $2", &[&id, &user.id]).await
}

/// Delete a note.
pub async fn delete_note(db: &Db, _user: &User, id: &str) -> Result<(), Error> {
    db.execute("delete from notes where id = $1", &[&id]).await
}

/// The caller's notes whose title starts with `prefix` (a request parameter).
pub async fn search_notes(db: &Db, user: &User, prefix: &str) -> Result<Vec<Note>, Error> {
    let sql = "select * from notes where owner = $1 and title like '".to_string() + prefix + "%'";
    db.query(&sql, &[&user.id]).await
}
