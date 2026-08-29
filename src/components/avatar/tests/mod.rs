use super::initials;

#[test]
fn initials_use_the_first_and_last_significant_name_parts() {
    assert_eq!(initials("Marlo Vlietstra"), "MV");
    assert_eq!(initials("Johan van der Brink"), "JB");
}

#[test]
fn unassigned_users_use_the_avatar_placeholder() {
    assert_eq!(initials("Unassigned"), "--");
}
