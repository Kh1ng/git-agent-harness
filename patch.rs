#[test]
fn colocated_installers_mutate_provider_yaml_safely() {
    for platform in ["linux", "macos"] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(home.join("MemoryCore/node_modules/yaml")).unwrap();
        std::fs::write(
            home.join("MemoryCore/package.json"),
            "{\"name\":\"MemoryCore\"}",
        ).unwrap();
        std::fs::write(
            home.join("MemoryCore/node_modules/yaml/index.js"),
            r#"module.exports = {
              parseDocument: function(str) {
                let obj = {};
                try { obj = JSON.parse(str); } catch(e) {}
                obj.llm = obj.llm || {};
                obj.embedding = obj.embedding || {};
                return {
                  setIn: function(path, val) {
                    if(path.length === 2) {
                      obj[path[0]] = obj[path[0]] || {};
                      obj[path[0]][path[1]] = val;
                    }
                  },
                  toString: function() { return JSON.stringify(obj); }
                }
              }
            };"#,
        ).unwrap();

        let source = script(&format!("install-{platform}.sh"));
        let start = source.find("# gateway-yaml-mutation:start\n").unwrap();
        let end = source[start..].find("# gateway-yaml-mutation:end").unwrap() + start;
        let block = &source[start..end];

        let run = |provider: &str, endpoint: &str, llm: &str, embed: &str| {
            let config_path = home.join("tdai-gateway.local.yaml");
            std::fs::write(&config_path, "{\"llm\":{}, \"embedding\":{}}").unwrap();
            let envs = vec![
                ("HOME", home.to_str().unwrap()),
                ("PATH", bin.to_str().unwrap()),
                ("GAH_GATEWAY_MEMORYCORE_PATH", home.join("MemoryCore").to_str().unwrap()),
                ("GAH_GATEWAY_PROVIDER", provider),
                ("GAH_GATEWAY_ENDPOINT", endpoint),
                ("GAH_GATEWAY_LLM_MODEL", llm),
                ("GAH_GATEWAY_EMBEDDING_MODEL", embed),
                ("gateway_local_config", config_path.to_str().unwrap()),
            ];
            let output = bash(&["-euc", block], &envs, true);
            assert!(output.status.success(), "{platform}: {}", text(&output));
            let content = std::fs::read_to_string(&config_path).unwrap();
            let json: serde_json::Value = serde_json::from_str(&content).unwrap();
            json
        };

        // Test ollama with explicit values
        let ollama = run("ollama", "http://test:11434", "my-llama", "my-embed");
        assert_eq!(ollama["llm"]["baseUrl"], "http://test:11434");
        assert_eq!(ollama["llm"]["model"], "my-llama");
        assert_eq!(ollama["embedding"]["provider"], "ollama");
        assert_eq!(ollama["embedding"]["baseUrl"], "http://test:11434");
        assert_eq!(ollama["embedding"]["model"], "my-embed");

        // Test openai with defaults
        let openai = run("openai", "", "", "");
        assert_eq!(openai["llm"]["baseUrl"], "https://api.openai.com/v1");
        assert_eq!(openai["llm"]["model"], "gpt-4o");
        assert_eq!(openai["embedding"]["provider"], "openai");
        assert_eq!(openai["embedding"]["baseUrl"], "https://api.openai.com/v1");
        assert_eq!(openai["embedding"]["model"], "text-embedding-3-small");
        
        // Test sed injection via query param on endpoint (issue #1319 regression)
        let ollama_injection = run("ollama", "https://example.test/v1?a=1&b=2", "llama3", "nomic-embed-text");
        assert_eq!(ollama_injection["llm"]["baseUrl"], "https://example.test/v1?a=1&b=2");
    }
}
