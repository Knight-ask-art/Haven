//! Versioned OPDS Basic credentials kept entirely in CredentialStore.
//! Legacy password-only records require an explicit username to be configured.

/// 规范格式的版本标记（载荷首行，精确匹配）。
pub const OPDS_BASIC_CREDENTIAL_PREFIX: &str = "haven-opds-basic-v1";

/// 载荷分隔符：换行。用户名与密码都不能包含它（用户名另受 RFC 7617 限制不得含 `:`）。
const FIELD_SEPARATOR: char = '\n';

/// 用户名长度上限（字符数）。比 `secret` 的 512 上限窄：一个真实账号名远比这短，
/// 而一个超长「用户名」只可能是把整条密码粘进了用户名框。
pub const OPDS_BASIC_USERNAME_MAX_CHARS: usize = 128;

/// 密码长度上限（字符数）。
pub const OPDS_BASIC_PASSWORD_MAX_CHARS: usize = 512;

#[derive(Clone, PartialEq, Eq)]
pub enum OpdsCredentialRead {
    /// 规范格式：可以直接构造 Basic `Authorization`。
    Basic { username: String, password: String },
    /// 历史明文密码（槽里没有格式标记）：**无法确定用户名**。
    ///
    /// 调用方必须停在这里，把来源投影成「需要重新配置凭据」，而不是猜一个用户名。
    LegacyPassword,
    /// 带格式标记但载荷本身非法（缺段、超长、用户名含分隔符）：防御性分支。
    ///
    /// 正常写入路径不可能产出它；一旦出现（外部改写、损坏的条目），一律按
    /// [`OpdsCredentialRead::LegacyPassword`] 同等对待——重新配置，不猜测。
    Malformed,
}

/// 用户名是否可以用在 Basic 载荷里。
///
/// RFC 7617 的 user-pass 里 `:` 是分隔符，因此用户名不得包含它；换行则是本格式自己的
/// 分隔符。其余字符照原样保留（包括空格与中文）：目录服务的账号名由对方决定，Haven
/// 不替它做字符白名单。
pub fn is_valid_opds_basic_username(username: &str) -> bool {
    !username.is_empty()
        && username.chars().count() <= OPDS_BASIC_USERNAME_MAX_CHARS
        && !username.contains(':')
        && !username.chars().any(char::is_control)
}

/// 密码是否可以编码进本格式。
///
/// 密码**允许**包含 `:`（这正是旧实现里会被静默切错的那类值），只排除分隔符本身。
pub fn is_valid_opds_basic_password(password: &str) -> bool {
    !password.is_empty()
        && password.chars().count() <= OPDS_BASIC_PASSWORD_MAX_CHARS
        && !password.contains(FIELD_SEPARATOR)
        && !password.contains('\r')
}

/// 编码一条规范格式载荷。调用方必须先用上面两个判定函数校验过输入。
pub fn encode_opds_basic_credential(username: &str, password: &str) -> String {
    format!("{OPDS_BASIC_CREDENTIAL_PREFIX}{FIELD_SEPARATOR}{username}{FIELD_SEPARATOR}{password}")
}

/// 解释一条已读出的 secret。
///
/// 只有**带精确前缀**的载荷才被当作新格式；其余一律是
/// [`OpdsCredentialRead::LegacyPassword`]。这里刻意不做任何启发式猜测。
pub fn read_opds_credential(secret: &str) -> OpdsCredentialRead {
    let Some(rest) = secret.strip_prefix(OPDS_BASIC_CREDENTIAL_PREFIX) else {
        return OpdsCredentialRead::LegacyPassword;
    };
    let Some(rest) = rest.strip_prefix(FIELD_SEPARATOR) else {
        // `haven-opds-basic-v1` 后面直接接了别的内容：不是本格式。
        return OpdsCredentialRead::LegacyPassword;
    };
    let mut parts = rest.splitn(2, FIELD_SEPARATOR);
    let (Some(username), Some(password)) = (parts.next(), parts.next()) else {
        return OpdsCredentialRead::Malformed;
    };
    if !is_valid_opds_basic_username(username) || !is_valid_opds_basic_password(password) {
        return OpdsCredentialRead::Malformed;
    }
    OpdsCredentialRead::Basic {
        username: username.to_owned(),
        password: password.to_owned(),
    }
}

/// Debug shows credential shape without exposing either account field.
///
/// 用它替代 `Debug` 派生，让「这个值有没有可能进日志」在类型层面就是否定的。
impl std::fmt::Debug for OpdsCredentialRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Basic { .. } => f.write_str("OpdsCredentialRead::Basic(<redacted>)"),
            Self::LegacyPassword => f.write_str("OpdsCredentialRead::LegacyPassword"),
            Self::Malformed => f.write_str("OpdsCredentialRead::Malformed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 规范格式必须是**可往返**的，且用户名里的 `:` 在读取侧不会切错。
    #[test]
    fn canonical_format_round_trips_and_rejects_colons_in_the_username() {
        let encoded = encode_opds_basic_credential("reader@example.org", "p@ss:word");
        assert_eq!(
            read_opds_credential(&encoded),
            OpdsCredentialRead::Basic {
                username: "reader@example.org".to_owned(),
                password: "p@ss:word".to_owned(),
            },
            "密码里的 `:` 必须原样保留，不能被当成用户名分隔符"
        );

        // RFC 7617 不允许用户名含 `:`；这里在写入前就拒绝，而不是编码出一个歧义载荷。
        assert!(!is_valid_opds_basic_username("user:name"));
        assert!(!is_valid_opds_basic_username(""));
        assert!(!is_valid_opds_basic_username(
            &"u".repeat(OPDS_BASIC_USERNAME_MAX_CHARS + 1)
        ));
        assert!(is_valid_opds_basic_username("读者"));
    }

    /// 历史明文密码**绝不**被猜测成 `用户名:密码`。
    #[test]
    fn legacy_plaintext_password_is_never_split_into_a_username() {
        for legacy in ["p@ss:word", "reader@example.org", "simplepassword"] {
            assert_eq!(
                read_opds_credential(legacy),
                OpdsCredentialRead::LegacyPassword,
                "没有格式标记的条目只能被判为历史密码，实际: {legacy}"
            );
        }
        // 只是**看起来**像前缀也不行：前缀必须精确匹配并跟一个分隔符。
        assert_eq!(
            read_opds_credential("haven-opds-basic-v1x\nuser\npass"),
            OpdsCredentialRead::LegacyPassword
        );
        assert_eq!(
            read_opds_credential("haven-opds-basic-v1user\npass"),
            OpdsCredentialRead::LegacyPassword
        );
    }

    /// 带标记但载荷残缺/超长：按「需要重新配置」处理，不产生半条凭据。
    #[test]
    fn marked_but_malformed_payload_never_yields_a_partial_credential() {
        for malformed in [
            format!("{OPDS_BASIC_CREDENTIAL_PREFIX}\nuser"),
            format!("{OPDS_BASIC_CREDENTIAL_PREFIX}\n\npass"),
            format!("{OPDS_BASIC_CREDENTIAL_PREFIX}\nuser\n"),
            format!(
                "{OPDS_BASIC_CREDENTIAL_PREFIX}\n{}\npass",
                "u".repeat(OPDS_BASIC_USERNAME_MAX_CHARS + 1)
            ),
        ] {
            assert_eq!(
                read_opds_credential(&malformed),
                OpdsCredentialRead::Malformed,
                "畸形载荷不得解析出半条凭据: {malformed}"
            );
        }
        assert!(!is_valid_opds_basic_password(
            &"p".repeat(OPDS_BASIC_PASSWORD_MAX_CHARS + 1)
        ));
        assert!(!is_valid_opds_basic_password(""));
    }

    /// 形状投影里不出现任何凭据材料——这是本模块替代 `Debug` 派生的目的。
    #[test]
    fn shape_projection_never_contains_credential_material() {
        let username = "sentinel-user-must-not-appear";
        let password = "sentinel-secret-must-not-appear";
        let read = read_opds_credential(&encode_opds_basic_credential(username, password));
        let rendered = format!("{read:?}");
        assert!(!rendered.contains(username), "{rendered}");
        assert!(!rendered.contains(password), "{rendered}");
        assert!(!format!("{:?}", OpdsCredentialRead::LegacyPassword).contains(username));
    }
}
