//! Library unit tests

#[cfg(test)]
mod types_tests {
    use crate::types::*;

    #[test]
    fn test_message_id_creation() {
        let id1 = MessageId::new();
        let id2 = MessageId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_message_flag_conversion() {
        let flag = MessageFlag::Seen;
        let imap_str = flag.to_imap_string();
        assert_eq!(imap_str, "\\Seen");

        let flag2 = MessageFlag::from_imap_string(&imap_str);
        assert_eq!(flag, flag2);
    }

    #[test]
    fn test_custom_flag() {
        let flag = MessageFlag::Custom("MyFlag".to_string());
        let imap_str = flag.to_imap_string();
        assert_eq!(imap_str, "MyFlag");

        let flag2 = MessageFlag::from_imap_string("MyFlag");
        assert_eq!(flag, flag2);
    }
}
