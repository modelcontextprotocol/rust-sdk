# しいたけ占い MCPサーバー

今週のしいたけ占いのアドバイスを踏まえて、LLMに回答を出力させるためのMCPサーバーです。  

## 事前準備

以下の通りコンテナイメージをビルドします。  

```bash
docker build -t mcp/shiitake-uranai-mcp:latest .
```

該当する星座を環境変数に設定してください。  

```bash
export CONSTELLATION=sagittarius
```

| 星座の値 | 日本語訳 |
|------|--------|
| aries | おひつじ座 |
| taurus | おうし座 |
| gemini | ふたご座 |
| cancer | かに座 |
| leo | しし座 |
| virgo | おとめ座 |
| libra | てんびん座 |
| scorpio | さそり座 |
| sagittarius | いて座 |
| capricorn | やぎ座 |
| aquarius | みずがめ座 |
| pisces | うお座 |

## 利用方法

### 1. Claude Desktopで利用する方法

`claude_desktop_config.json` に以下の通り記載してください。  
`CONSTELLATION` には上記した該当する星座の値を入力してください。  

mac

```json
{
    "mcpServers": {
        "shiitake-uranai-mcp": {
            "command": "docker",
            "args": [
                "run",
                "-i",
                "--rm",
                "-e",
                "CONSTELLATION",
                "mcp/shiitake-uranai-mcp"
            ],
            "env": {
                "CONSTELLATION": "sagittarius"
            }
        }
    }
}
```

windows

```json
{
    "mcpServers": {
        "shiitake-uranai-mcp": {
            "command": "wsl.exe",
            "args": [
                "bash",
                "-c",
                "docker run -i --rm -e CONSTELLATION mcp/shiitake-uranai-mcp"
            ],
            "env": {
                "CONSTELLATION": "sagittarius"
            }
        }
    }
}
```


### 2. Dockerのインタラクティブモードで利用する方法

コンテナを起動します

```shell
docker run -i --rm -e CONSTELLATION mcp/shiitake-uranai-mcp
```

ツール一覧取得のためのリクエスト

```json
{"method": "tools/list", "jsonrpc": "2.0", "id": 1}
```

今週の占いの取得のためのリクエスト

```json
{"method": "tools/call", "params": { "name": "fetch_fortune", "arguments": null }, "jsonrpc": "2.0", "id": 2}
```