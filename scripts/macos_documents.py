"""Launch Services document declarations; importing this module has no side effects."""


def document_types():
    extensions = "txt text log md markdown csv tsv json jsonl ndjson jsonc yaml yml toml ini cfg conf xml html htm css js jsx ts tsx py rs sh bash zsh sql c h cpp hpp java go rb php swift kt kts vue svelte diff patch env properties dockerfile".split()
    return {
        "CFBundleDocumentTypes": [
            {
                "CFBundleTypeName": "Text and source documents",
                "CFBundleTypeRole": "Editor",
                "LSHandlerRank": "Alternate",
                "LSItemContentTypes": ["public.text", "public.source-code", "public.json", "public.xml", "com.heartfilia.litetext.jsonl", "com.heartfilia.litetext.text"],
                "CFBundleTypeExtensions": extensions,
            }
        ],
        "UTImportedTypeDeclarations": [
            {
                "UTTypeIdentifier": "com.heartfilia.litetext.jsonl",
                "UTTypeDescription": "JSON Lines document",
                "UTTypeConformsTo": ["public.plain-text"],
                "UTTypeTagSpecification": {
                    "public.filename-extension": ["jsonl", "ndjson"],
                    "public.mime-type": ["application/x-ndjson", "application/jsonl"],
                },
            },
            {
                "UTTypeIdentifier": "com.heartfilia.litetext.text",
                "UTTypeDescription": "Text and configuration document",
                "UTTypeConformsTo": ["public.plain-text"],
                "UTTypeTagSpecification": {"public.filename-extension": [ext for ext in extensions if ext not in {"jsonl", "ndjson"}]},
            },
        ],
    }
