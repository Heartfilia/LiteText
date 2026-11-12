import plistlib
import unittest

from macos_documents import document_types


class DocumentTypesTest(unittest.TestCase):
    def test_plist_declares_json_lines_and_common_text_without_claiming_default(self):
        data = plistlib.loads(plistlib.dumps(document_types()))
        documents = data["CFBundleDocumentTypes"]
        extensions = {ext for doc in documents for ext in doc["CFBundleTypeExtensions"]}
        self.assertTrue({"jsonl", "ndjson", "log", "txt", "json", "md", "py", "rs", "yaml", "toml", "csv"} <= extensions)
        for doc in documents:
            self.assertEqual(doc["LSHandlerRank"], "Alternate")
            self.assertEqual(doc["CFBundleTypeRole"], "Editor")
        types = {item["UTTypeIdentifier"]: item for item in data["UTImportedTypeDeclarations"]}
        jsonl = types["com.heartfilia.litetext.jsonl"]
        self.assertIn("public.plain-text", jsonl["UTTypeConformsTo"])
        self.assertEqual(jsonl["UTTypeTagSpecification"]["public.filename-extension"], ["jsonl", "ndjson"])


if __name__ == "__main__":
    unittest.main()
