with open("src/mcp/serve.rs", "r") as f:
    content = f.read()

# find the last occurrence of "}" before the test we added
# actually, it's easier to just find "    }\n}\n\n\n    #[tokio::test]"
import re
content = re.sub(r'    }\n}\n\n\n    #\[tokio::test\]', r'    }\n\n    #[tokio::test]', content)

# ensure the file ends with a single "}"
content = re.sub(r'        let response = router.oneshot\(request\).await.unwrap\(\);\n}$', r'        let response = router.oneshot(request).await.unwrap();\n        assert_eq!(response.status(), StatusCode::OK);\n    }\n}\n', content)

with open("src/mcp/serve.rs", "w") as f:
    f.write(content)
