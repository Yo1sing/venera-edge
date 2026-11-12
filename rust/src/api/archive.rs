use anyhow::Result;

pub fn read_ehviewer_library(database: String) -> Result<String> {
    use serde_json::json;
    let db = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut labels = vec![String::new()];
    labels.extend(
        db.prepare("SELECT LABEL FROM DOWNLOAD_LABELS ORDER BY TIME DESC")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?,
    );
    let mut groups = Vec::new();
    for label in labels {
        let mut statement=db.prepare("SELECT DN.DIRNAME,DL.TITLE_JPN,DL.TITLE,DL.TIME,DL.CATEGORY FROM DOWNLOAD_DIRNAME DN JOIN DOWNLOADS DL ON DL.GID=DN.GID WHERE ((?1='' AND DL.LABEL IS NULL) OR DL.LABEL=?1) AND DL.STATE=3 ORDER BY DL.TIME DESC")?;
        let comics=statement.query_map([&label],|row| {
            let japanese=row.get::<_,Option<String>>(1)?.unwrap_or_default();
            let title=if japanese.is_empty(){row.get::<_,String>(2)?}else{japanese};
            let category=row.get::<_,u32>(4)?.checked_ilog2().unwrap_or(0) as usize;
            let tag=["MISC","DOUJINSHI","MANGA","ARTISTCG","GAMECG","IMAGE SET","COSPLAY","ASIAN PORN","NON-H","WESTERN"].get(category).unwrap_or(&"MISC");
            let time=row.get::<_,i64>(3)?;
            Ok(json!({"directory":row.get::<_,String>(0)?,"title":title,"tags":[tag],"created_at":if time==0{crate::cache::now_ms()}else{time}}))
        })?.collect::<rusqlite::Result<Vec<_>>>()?;
        groups.push(json!({"label":label,"comics":comics}));
    }
    Ok(serde_json::to_string(&groups)?)
}
pub fn import_comic_archive(
    input: String,
    cache: String,
    local: String,
    database: String,
) -> Result<String> {
    crate::comic_archive::import(&input, &cache, &local, &database)
}
pub fn extract_archive(input: String, output: String) -> Result<()> {
    crate::archives::extract(input.as_ref(), output.as_ref())
}
pub fn compress_folder(input: String, output: String) -> Result<()> {
    crate::archives::compress_folder(input.as_ref(), output.as_ref())
}
