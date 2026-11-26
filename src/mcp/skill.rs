//! The yrem skill served over MCP (SEP-2640 Skills extension), for clients
//! that load skills from servers instead of from a plugin.

use rmcp::model::{
    ErrorData, GetSkillResult, ListSkillsResult, ReadResourceResult, Resource, ResourceContents,
    Skill, SkillFrontmatter, SkillResource, SkillResources,
};

/// The same file the plugins copy (tests/plugins.rs keeps them identical).
pub(super) const SKILL_MD: &str = include_str!("../../skills/yrem/SKILL.md");
pub(super) const SKILL_URI: &str = "skill://yrem/SKILL.md";
const MIME: &str = "text/markdown";

/// `name` and `description` from the `---` frontmatter block of [`SKILL_MD`].
fn frontmatter() -> SkillFrontmatter {
    let block = SKILL_MD
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---"))
        .map_or("", |(block, _)| block);
    let field = |key: &str| {
        block
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
            .map_or_else(String::new, |v| v.trim().to_owned())
    };
    SkillFrontmatter::new(field("name"), field("description"))
}

fn skill() -> Skill {
    Skill::new(
        SKILL_URI,
        frontmatter(),
        SkillResources::Files(vec![SkillResource::from_bytes(
            SKILL_URI,
            SKILL_MD.as_bytes(),
        )]),
    )
}

pub(super) fn list_skills() -> ListSkillsResult {
    ListSkillsResult::with_all_items(vec![skill()])
}

/// # Errors
/// `-32602` for any URI but [`SKILL_URI`], as SEP-2640 requires.
pub(super) fn get_skill(uri: &str) -> Result<GetSkillResult, ErrorData> {
    if uri == SKILL_URI {
        Ok(GetSkillResult::new(skill()))
    } else {
        Err(ErrorData::invalid_params(
            format!("no skill at {uri}"),
            None,
        ))
    }
}

pub(super) fn resources() -> Vec<Resource> {
    vec![
        Resource::new(SKILL_URI, "yrem skill")
            .with_mime_type(MIME)
            .with_size(SKILL_MD.len() as u64),
    ]
}

/// # Errors
/// Resource not found for any URI but [`SKILL_URI`].
pub(super) fn read(uri: &str) -> Result<ReadResourceResult, ErrorData> {
    if uri == SKILL_URI {
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(SKILL_MD, SKILL_URI).with_mime_type(MIME),
        ]))
    } else {
        Err(ErrorData::resource_not_found(
            format!("no resource at {uri}"),
            None,
        ))
    }
}
