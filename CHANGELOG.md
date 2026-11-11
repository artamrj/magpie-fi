# Changelog

What changed in each magpie-fi release. The newest is first.

## [0.10.0](https://github.com/artamrj/magpie-fi/releases/tag/v0.10.0) - 2026-11-11

### ✨ New
- Review stays quick with thousands of songs waiting, and can be searched ([8a3a25d](https://github.com/artamrj/magpie-fi/commit/8a3a25d1c6412b8c8a7dcf72154b41cb0781307c))
- Downloaded covers are kept as files, not inside the database ([db0b8da](https://github.com/artamrj/magpie-fi/commit/db0b8dae630650541acfcd6af9afcb644666b221))

### 🐛 Fixes
- The Artists page stays quick with thousands of artists, and a song gets a second look after a source was busy ([7f2beb8](https://github.com/artamrj/magpie-fi/commit/7f2beb8fb230566cdf15a292a1ee18971900fdac))

### 📖 Docs
- The README shows what magpie-fi does, with screenshots and a quick start ([fb693ba](https://github.com/artamrj/magpie-fi/commit/fb693baec14303a90767d64e785b26408a3f5db3))

## [0.9.1](https://github.com/artamrj/magpie-fi/releases/tag/v0.9.1) - 2026-11-11

### 🐛 Fixes
- A store's "- Persian Music" ending no longer splits an album in two ([f38a484](https://github.com/artamrj/magpie-fi/commit/f38a4840256a4cf8dc07f5161fe0b28060fd3084))
- A wrongly tagged song is no longer filed on its own, and a crash or restart no longer leaves work stuck ([210e6b3](https://github.com/artamrj/magpie-fi/commit/210e6b3e02004a819120d4d0e95ceeea9bdadf9d))

## [0.9.0](https://github.com/artamrj/magpie-fi/releases/tag/v0.9.0) - 2026-11-11

### ✨ New
- Review can keep a file's own tags when no match is right ([f080640](https://github.com/artamrj/magpie-fi/commit/f080640a2755a8b56fd553b0dee7a7bf5f038d8f))
- Keeping a file's own tags also keeps its own cover when it is a real one ([276c8f6](https://github.com/artamrj/magpie-fi/commit/276c8f6f8554975b86500b4ab68bfb8eecc10023))

### 🐛 Fixes
- Songs whose file you deleted from the inbox leave Review, and Remove works on them ([ed76a0e](https://github.com/artamrj/magpie-fi/commit/ed76a0e1066a2dd37ec9610c2b84e290fe4dd097))
- A match no longer drops the file's co-singers, and Apple Music and Deezer links fill in the tags ([091ebe2](https://github.com/artamrj/magpie-fi/commit/091ebe2d5c607e6f32dbb12b732e75d44228caa9))
- Songs of one new album filed together land in one album when sources spell it differently ([faa9583](https://github.com/artamrj/magpie-fi/commit/faa95833bee30877e9f278e768261f4456672f32))
- The cover search finds covers of songs that are only sold on an album ([f69a33f](https://github.com/artamrj/magpie-fi/commit/f69a33f04f22bcd7ad27102c97af74116b7a948f))

## [0.8.3](https://github.com/artamrj/magpie-fi/releases/tag/v0.8.3) - 2026-11-11

### ✨ New
- **web:** The sidebar shows which version is running, next to the name ([d24d240](https://github.com/artamrj/magpie-fi/commit/d24d240abe46818a8a6b0c50371fb91d8c3a7f20))
- **web:** The Song page shows each artist a song is credited to ([fb22363](https://github.com/artamrj/magpie-fi/commit/fb22363bc6edaa074919a88e39b9cfd69d4d9101))

### 🐛 Fixes
- Sending a link again fetches a song you removed in Review ([6b62f19](https://github.com/artamrj/magpie-fi/commit/6b62f19c517134710b9ed92b1ef7c33b1ef6c6ce))
- **web:** The web page's tools install with no security warnings ([cd56dff](https://github.com/artamrj/magpie-fi/commit/cd56dff7aa8c19bc3a71b4e92e7fd4660b448ff1))
- Drop SoundCloud search, which needs a key SoundCloud no longer gives out ([90e7463](https://github.com/artamrj/magpie-fi/commit/90e74632a41210febeac32e079adf670e2fb28d5))
- **web:** The favicon and app icons use the same blue as the logo in the sidebar ([dd63667](https://github.com/artamrj/magpie-fi/commit/dd636679de885faf221a51080253d0134d0904c1))
- **web:** The magpie sits in the middle of its icon and fills it better ([75cfe8d](https://github.com/artamrj/magpie-fi/commit/75cfe8dcf1b1e5a8ae9b8230aa8ffb4cc7087f66))

## [0.8.2](https://github.com/artamrj/magpie-fi/releases/tag/v0.8.2) - 2026-11-11

### ✨ New
- Releases come with real notes and take one command ([4af5abd](https://github.com/artamrj/magpie-fi/commit/4af5abd94e4c421163c9c5293b3ecfc89da3be89))

## [0.8.1](https://github.com/artamrj/magpie-fi/releases/tag/v0.8.1) - 2026-11-11

### ✨ New
- **web:** The magpie logo is the icon everywhere: browser tab, iPhone and Android home screen ([c6796fe](https://github.com/artamrj/magpie-fi/commit/c6796fe2c7e85797091e499fe5012d3792413039))

### 🐛 Fixes
- A song sent as an Apple Music or Shazam link is found by its own number ([24a5181](https://github.com/artamrj/magpie-fi/commit/24a5181e5a375b7c8ac7a5389611f2f70c1cae41))

## [0.8.0](https://github.com/artamrj/magpie-fi/releases/tag/v0.8.0) - 2026-11-11

### ✨ New
- Radio Javan podcasts (DJ mixes) are recognised and can be added by link ([dd646a6](https://github.com/artamrj/magpie-fi/commit/dd646a692aa519e021c13b537026b17cc48aedfd))
- A pasted music video downloads the studio song, and YouTube Music album links work ([b502956](https://github.com/artamrj/magpie-fi/commit/b502956f700d740a4937ee81b5d3e4f2e65a91a5))
- **web:** Magpie-fi has its own logo, a magpie on its nest with a song ([cf0fc55](https://github.com/artamrj/magpie-fi/commit/cf0fc550c770a4d5a6efef65b33fa90fd9b64309))

### 🐛 Fixes
- A song keeps its main singer when sources spell the singers shorter ([f446073](https://github.com/artamrj/magpie-fi/commit/f4460735a3497bf124e8d4684e8ff5554c4ce041))
- An album stays one album in Navidrome, and albums already split can be made one ([d13f220](https://github.com/artamrj/magpie-fi/commit/d13f22080e977a1151df5df3da6a68e1d4c0f2aa))
- MP3s keep their MusicBrainz album, release-group and track IDs ([7c4cadf](https://github.com/artamrj/magpie-fi/commit/7c4cadf405a28d001861646cfdf2224550b96009))
- **web:** The sidebar status looks like the sidebar and shows only problems ([bcbc045](https://github.com/artamrj/magpie-fi/commit/bcbc045d7c64e6f98f60800572990248edd6ebfd))

## [0.6.0](https://github.com/artamrj/magpie-fi/releases/tag/v0.6.0) - 2026-11-10

### ✨ New
- A Files tab to browse, search and look at every song in the library ([bc6dcb9](https://github.com/artamrj/magpie-fi/commit/bc6dcb95b695ff174c7aa426b43cfe23d59c5569))
- Fix a song's tags by hand on the Files tab, with Undo ([3ed7271](https://github.com/artamrj/magpie-fi/commit/3ed72719c885ed59e7360f14d5e52745e8fa808c))
- Edit all songs of an album folder at once on the Files tab ([d289cf2](https://github.com/artamrj/magpie-fi/commit/d289cf284f4de3281e696d5cc2b232601cdca3b6))
- **scan:** Enhance song identification with ISRC handling and listening logic ([a263856](https://github.com/artamrj/magpie-fi/commit/a263856f1333668e60d17427a64e998a2d2e9b3d))
- A sure match without a cover keeps its tags while a cover finder looks for one ([481a35f](https://github.com/artamrj/magpie-fi/commit/481a35f4ccd11e198011b459ab00ee5de471244e))
- **web:** The Review page fills a big laptop or desktop screen ([23267c0](https://github.com/artamrj/magpie-fi/commit/23267c09a0a9e3c2dabf23c4e783c9a99b3d28d0))
- Add launch configuration for backend and web development ([9dfa18c](https://github.com/artamrj/magpie-fi/commit/9dfa18c0377779fdea4701615facd29c5499b8bf))
- **web:** The Library tab is now Artists, and the status sits in the sidebar ([e2be094](https://github.com/artamrj/magpie-fi/commit/e2be094877e3a857eb3a554b5065b6e68e942539))

### 🐛 Fixes
- A song keeps its main singer when a source lists only the featured singers ([66e1b77](https://github.com/artamrj/magpie-fi/commit/66e1b77adf0004abd333d10dc5b5e714cb5c76af))
- **docs:** Update port accessibility and login details in documentation ([567d145](https://github.com/artamrj/magpie-fi/commit/567d1454232daba5526d6ff0164520db2c606f33))
- A Persian title spelled another way by a source is the same song, and old albums keep their earliest year ([5ff5987](https://github.com/artamrj/magpie-fi/commit/5ff59871be0ac5098570574aa6e0a1f11a6d910d))

## [0.5.1](https://github.com/artamrj/magpie-fi/releases/tag/v0.5.1) - 2026-11-10

### ✨ New
- **web:** Seek anywhere in a song from the song screen ([ec8864e](https://github.com/artamrj/magpie-fi/commit/ec8864e12a0551f9552489b1498f8f7a5ec6d1dd))

### 📖 Docs
- Add AGENTS.md for AI coding agents ([dc34718](https://github.com/artamrj/magpie-fi/commit/dc34718ab3a89c175312a5cb4e7906e5d9a9cc5c))

## [0.5.0](https://github.com/artamrj/magpie-fi/releases/tag/v0.5.0) - 2026-11-10

### ✨ New
- Write the library the way Navidrome reads it ([a590d62](https://github.com/artamrj/magpie-fi/commit/a590d62a707c51fccc95012d42894d8b06f0e09f))

## [0.4.5](https://github.com/artamrj/magpie-fi/releases/tag/v0.4.5) - 2026-11-10

### 🐛 Fixes
- **web:** The busy strip's spinner no longer stretches across the strip ([7014d73](https://github.com/artamrj/magpie-fi/commit/7014d737c1d2764b779d7c5ecf5341db4ca19032))

## [0.4.4](https://github.com/artamrj/magpie-fi/releases/tag/v0.4.4) - 2026-11-10

### 🐛 Fixes
- The file decides who is the main singer of a duet or featured song ([c533b61](https://github.com/artamrj/magpie-fi/commit/c533b6192a3872e3529607bd01e058e14ae5ff21))

## [0.4.3](https://github.com/artamrj/magpie-fi/releases/tag/v0.4.3) - 2026-11-10

### ✨ New
- Add screen as a short downloads-style list; the same link is not asked for twice ([332446b](https://github.com/artamrj/magpie-fi/commit/332446b2baec69b6b447de105aeda2727a9c0112))

## [0.4.2](https://github.com/artamrj/magpie-fi/releases/tag/v0.4.2) - 2026-11-10

### 🐛 Fixes
- **web:** Show changes without a reload ([ece36b4](https://github.com/artamrj/magpie-fi/commit/ece36b4c187367c43faa910dc09a3d5ec124b086))

## [0.4.1](https://github.com/artamrj/magpie-fi/releases/tag/v0.4.1) - 2026-11-10

### 🐛 Fixes
- Find Deezer tracks the filtered search misses; show the file's own tags in Review ([e94d9a2](https://github.com/artamrj/magpie-fi/commit/e94d9a2968a2377056e30fca6d762b3f6ed2c869))

## [0.4.0](https://github.com/artamrj/magpie-fi/releases/tag/v0.4.0) - 2026-11-10

### ✨ New
- **web:** Redesign the page as a Telegram Mini App ([f4d06cc](https://github.com/artamrj/magpie-fi/commit/f4d06ccbd21cad7ce682cb7bd2f96c4a633a0bf3))
- **telegram:** Review questions open the song in the Mini App ([60cc54f](https://github.com/artamrj/magpie-fi/commit/60cc54f0b6eb11a9a4fd63e3572af7168a06b5b3))

## [0.3.3](https://github.com/artamrj/magpie-fi/releases/tag/v0.3.3) - 2026-11-10

### 🐛 Fixes
- Keep featured artists out of artist names and folders ([cfb9dd6](https://github.com/artamrj/magpie-fi/commit/cfb9dd6d6323b81ee9e152b186b81f717aeafbdf))

## [0.3.2](https://github.com/artamrj/magpie-fi/releases/tag/v0.3.2) - 2026-11-10

### 🔧 Behind the scenes
- Use only free sources ([e8eeb1f](https://github.com/artamrj/magpie-fi/commit/e8eeb1fff75b7630c0c544d0f1b6deca84b4b1af))

## [0.3.1](https://github.com/artamrj/magpie-fi/releases/tag/v0.3.1) - 2026-11-10

### 🐛 Fixes
- Shazam links, Telegram tags, remix and cover matching, silent duplicates ([5cce851](https://github.com/artamrj/magpie-fi/commit/5cce85125dd4c4b4d6e6ac382bf331ea6808b145))

### 📦 Updated tools and libraries
- **deps:** Bump library/rust ([2ee3c6d](https://github.com/artamrj/magpie-fi/commit/2ee3c6d09d0bde85286ffd032c456f9fa4e5809e))
- **deps:** Bump the frontend-dependencies group across 1 directory with 8 updates ([05a318d](https://github.com/artamrj/magpie-fi/commit/05a318dd30114ad2e91d6b6ba8ec0f6340668a10))

## [0.3.0](https://github.com/artamrj/magpie-fi/releases/tag/v0.3.0) - 2026-11-10

### ✨ New
- One identity per artist, duets, duplicates by sound, Artists page ([8b85c9b](https://github.com/artamrj/magpie-fi/commit/8b85c9b1069a5735987247a91adaf5c0d8e13a50))

## [0.2.0](https://github.com/artamrj/magpie-fi/releases/tag/v0.2.0) - 2026-11-10

### 🐛 Fixes
- Covers, Mini App and sources; one file per song ([e293d98](https://github.com/artamrj/magpie-fi/commit/e293d980e5bd7c2dc4c330d4408aeca982dc7157))

## [0.1.0](https://github.com/artamrj/magpie-fi/releases/tag/v0.1.0) - 2026-11-10

### ✨ New
- Add audio metadata reading and processing ([029e4ec](https://github.com/artamrj/magpie-fi/commit/029e4ec32f4270466fc792e039e238aec0d63673))
- Enhance configuration and validation for user settings ([5e17973](https://github.com/artamrj/magpie-fi/commit/5e17973afa096cff7650680ad4ae3ff57e50efac))
- Implement workflow management and enhance scanning process ([ec4db0f](https://github.com/artamrj/magpie-fi/commit/ec4db0fc86aab2cf89894c7c3efe52d0753efdb3))
- Enhance terminal logging and metadata handling in workflow ([4ecc93c](https://github.com/artamrj/magpie-fi/commit/4ecc93c0d4fc2179de3f8d5fcf82d3e94bc517a0))
- Update directory structure to use .local for local runtime data ([116eda8](https://github.com/artamrj/magpie-fi/commit/116eda8852df4d51393b59321db9c6661e931212))
- Update deployment and development documentation for local runtime data management ([e9c1bc1](https://github.com/artamrj/magpie-fi/commit/e9c1bc1377dd146a31bf478a324f146cb1e3a20c))
- Add architecture documentation for Ununknown application ([687c6b8](https://github.com/artamrj/magpie-fi/commit/687c6b8621a005a9c1b5ffe56cf57a068a80c00b))
- Add concurrency settings and validation for metadata processing in configuration ([dd16be4](https://github.com/artamrj/magpie-fi/commit/dd16be42de3b1275ff37397c96e26b95ea07c760))
- Add concurrency settings for scan workers, artwork downloads, and tag writers ([de90efc](https://github.com/artamrj/magpie-fi/commit/de90efc61e195cd7a1d428e69d252f60fa4779ad))
- Implement provider caching for AcoustID and MusicBrainz API responses ([19ade56](https://github.com/artamrj/magpie-fi/commit/19ade564b53f6507faadf1584a5814caaf8fe2ab))
- Enhance error handling with typed ApiError variants and specific HTTP status codes ([8c37b55](https://github.com/artamrj/magpie-fi/commit/8c37b55402850a9c55a5a232a20917d3f3ea87ce))
- Enhance terminal logging with detailed error and context fields ([4bcc11e](https://github.com/artamrj/magpie-fi/commit/4bcc11ea900189115985726c015098659f4ede05))
- Add Docker and docker-compose configurations for local and deploy environments ([f3ceda1](https://github.com/artamrj/magpie-fi/commit/f3ceda19675eb64803b4e0ab71435d32a3ce2a94))
- Add environment variable configuration for AcoustID and MusicBrainz in .env.example and update docker-compose setup ([465b153](https://github.com/artamrj/magpie-fi/commit/465b1531b885ea9a359850c3f0e5dc810b516ff0))
- Enhance error handling for artwork retrieval with specific not found responses ([5c9735e](https://github.com/artamrj/magpie-fi/commit/5c9735ebc0992ff61c225a3f3f387d982616c805))
- Update Flow component and styles for improved accessibility and layout ([c510695](https://github.com/artamrj/magpie-fi/commit/c510695e87b095202e9022c0c6588273b2f43b8e))
- Refactor MusicMetadataCard and PreviewRow for improved layout and accessibility; update styles for consistency ([44e8f25](https://github.com/artamrj/magpie-fi/commit/44e8f25bdf20c33a52b7c858520ab96619be65a7))
- Refactor MusicMetadataCard and PreviewRow components for cleaner code; update styles for improved layout and consistency ([d54efb3](https://github.com/artamrj/magpie-fi/commit/d54efb3d16f4814e1eb650d85542d499f431a170))
- Replace PreviewVirtualList with PreviewList for improved rendering; update styles for consistency and layout ([9665c75](https://github.com/artamrj/magpie-fi/commit/9665c7591ee499b85d1475bca9c394c20fd3cb05))
- Improve error logging in process_file; enhance candidate selection logic for manual mode ([9c54bf2](https://github.com/artamrj/magpie-fi/commit/9c54bf2aece487465f46ddeca7a6195083fe832d))
- Update version to 0.5.0 across all relevant files; adjust user agent and documentation accordingly ([dd19270](https://github.com/artamrj/magpie-fi/commit/dd1927083467815f6245307cc4a254d914a81aa8))
- Update MusicBrainz user agent to version 0.5.0 in .env.example ([732ba9d](https://github.com/artamrj/magpie-fi/commit/732ba9d8a09deb27eb9a612ec54b320106e9df55))
- Add comprehensive backend documentation including architecture, API, configuration, database, developer guide, and workflows ([d7f3710](https://github.com/artamrj/magpie-fi/commit/d7f371017182ac1f012d643e2dc47793352efb95))
- Implement fingerprint caching mechanism to optimize file fingerprinting process ([7efa5bc](https://github.com/artamrj/magpie-fi/commit/7efa5bc0fb243cbeffab4cdaf611e9037fc14d68))
- Implement new workflow components and views ([f3fa8fe](https://github.com/artamrj/magpie-fi/commit/f3fa8fe021f6d9f800ecb569076f305afb3ce06d))
- Enhance track review with error handling and total count display ([c9f9f3b](https://github.com/artamrj/magpie-fi/commit/c9f9f3b086098bf11baaaaaa38ef57083d8851b1))
- Enhance candidate metadata handling and review process ([b003b6c](https://github.com/artamrj/magpie-fi/commit/b003b6cfd1929f1260b8dded715566be99398701))
- Enhance settings and provider management ([0fa57ed](https://github.com/artamrj/magpie-fi/commit/0fa57ed7976c4f980cf6a15cb85e2033798d11d9))
- Add Discogs and Last.fm providers for music search ([d0ab6d9](https://github.com/artamrj/magpie-fi/commit/d0ab6d92d1f074ad799b7de514f335a8b9f843ea))
- Enhance event handling and provider querying with improved error management and deduplication ([742c8d7](https://github.com/artamrj/magpie-fi/commit/742c8d73d1f1f68dd062cc217febc799e550ead5))
- Add JSON settings editor with validation and formatting options ([ea73c60](https://github.com/artamrj/magpie-fi/commit/ea73c6002d77f858ee3f86c4e951295c228d0264))
- Enhance candidate processing and error handling, add static file fallback service ([bd95f75](https://github.com/artamrj/magpie-fi/commit/bd95f75421710afd3fb85ee96ccca51087b61e71))
- **dev:** Add development script for backend and frontend setup ([2022214](https://github.com/artamrj/magpie-fi/commit/2022214e993dc91e44ddb1f21b5296cee0d230d0))
- Feat(itunes): add iTunes provider for music catalog search ([958b8d6](https://github.com/artamrj/magpie-fi/commit/958b8d6e2f8ff19784dbdc25a0be1e48d4130ed0))
- **scan:** Improve candidate matching and metadata handling ([0b06501](https://github.com/artamrj/magpie-fi/commit/0b06501fd3d486ffb6b081b2102da9f5d9def13a))
- **apply:** Enhance destination naming and handle duplicate filenames ([8a32548](https://github.com/artamrj/magpie-fi/commit/8a325481c841db2365debf18d6cb83f78ef61ee0))
- **genre:** Enrich candidate genres with artist data from Wikidata ([0508538](https://github.com/artamrj/magpie-fi/commit/0508538942827d5fe9a4dcb20b7119e93062f37f))
- **replaygain:** Implement ReplayGain analysis and metadata writing ([5d06d0b](https://github.com/artamrj/magpie-fi/commit/5d06d0b1dc9b6fc2210d084767d9338b09942567))
- **integrity:** Add audio integrity checking and reporting for damaged files ([d34cc99](https://github.com/artamrj/magpie-fi/commit/d34cc9937d3c16c13949ec5729b32827f4a4b313))
- **setup:** Add option to remove source files after successful output ([ebcdf50](https://github.com/artamrj/magpie-fi/commit/ebcdf50e5341a04e2ac7de77f2631a8fbc36a6ed))
- **spotify, audd, youtube:** Integrate audio recognition and metadata providers ([fb8d31c](https://github.com/artamrj/magpie-fi/commit/fb8d31c90d336c6965b30255c2e67a2171a0dedd))
- **artwork:** Enhance artwork handling and metadata integration ([ee77478](https://github.com/artamrj/magpie-fi/commit/ee77478b95790f754f8bb540bb20032c3a874d0b))
- **tracks:** Add endpoint to update track artwork from URL ([ba44b54](https://github.com/artamrj/magpie-fi/commit/ba44b54477fdd97eca714945daad31189dfb0dc2))
- **source:** Add endpoint to resolve YouTube source URLs and update UI ([053f0e4](https://github.com/artamrj/magpie-fi/commit/053f0e40ca6737cf334dae96b83aaa7cc78b8d49))
- **tracks:** Add artwork preview endpoint and improve artwork handling ([22e258d](https://github.com/artamrj/magpie-fi/commit/22e258d6fc3a12cdc5f9830d254556b735d24a7f))
- **credits:** Implement normalization of featured artist credits ([52be631](https://github.com/artamrj/magpie-fi/commit/52be63198ceb8c6a296ce0da439ef70b3c84736f))
- **tag_writer:** Implement resilient tag writing and legacy metadata cleanup ([7ed792f](https://github.com/artamrj/magpie-fi/commit/7ed792fe65f7dc0c94e4fc4a3320aaef2a67ca7f))
- **soundcloud:** Integrate SoundCloud API for track metadata retrieval ([9e1bcea](https://github.com/artamrj/magpie-fi/commit/9e1bcea0a059943800e92ad2c847a49d0e123bd1))
- **artwork:** Implement artwork override handling and new endpoints ([f674b93](https://github.com/artamrj/magpie-fi/commit/f674b93f550b64fc8945aac2b720bf8d328284a2))
- **credits:** Enhance artist and album artist normalization for candidates ([17eeeef](https://github.com/artamrj/magpie-fi/commit/17eeeef1e280c88796e94cff40900ddccbacfc37))
- **tracks:** Add audio streaming endpoint for track playback ([057b97e](https://github.com/artamrj/magpie-fi/commit/057b97e16d1ed0fa33cb54e44d9d95a849a8c35e))
- **review:** Enhance candidate display with source information and links ([b327832](https://github.com/artamrj/magpie-fi/commit/b32783284313b443674bdf8b8dc9cf27144fd345))
- **review:** Enhance candidate metadata handling in manual editor ([3cc8533](https://github.com/artamrj/magpie-fi/commit/3cc853377237aa2c04a3d206c97b982c94c4f100))
- Add smart auto-select for review tracks ([a579359](https://github.com/artamrj/magpie-fi/commit/a5793593a25ea5c3ef3ab3ceca8dd797a7ab7727))
- **tracks:** Add undo identification feature for track review ([3cfe896](https://github.com/artamrj/magpie-fi/commit/3cfe896963a5ce3973eb186bb1a6ec56d64e71ab))
- **radiojavan:** Integrate Radio Javan provider for metadata retrieval ([76b97a0](https://github.com/artamrj/magpie-fi/commit/76b97a0eb16da67e5bcd909b1a45c492847c25bd))
- **tag_writer:** Enhance artwork verification for FLAC files ([2f62eff](https://github.com/artamrj/magpie-fi/commit/2f62eff1bce9ba7c5fae4b5fc6a17940afa3f5dd))
- **scan_pipeline:** Integrate smart approval and metadata completion ([87a77f5](https://github.com/artamrj/magpie-fi/commit/87a77f5a591f24cabba34bdaf7eb6342c38724bd))
- Feat: add smart auto-select for review tracks ([8889f73](https://github.com/artamrj/magpie-fi/commit/8889f73618e949860545c6299028310f19567f79))
- **genius:** Remove access token requirement and enhance metadata retrieval ([a7f3651](https://github.com/artamrj/magpie-fi/commit/a7f3651f3f057ed3ba60f8ffc5d9d959d8f7cc77))
- **track-inspector:** Enhance candidate comparison UI and scoring details ([c351aa3](https://github.com/artamrj/magpie-fi/commit/c351aa3525e53404f19e965834782c3d13973b89))
- **scan:** Add retry mechanism for handling problematic files ([07fdce9](https://github.com/artamrj/magpie-fi/commit/07fdce916bb5ad2ca9acfa0c27375ee1521a909a))
- **api:** Add original artwork preview endpoint for tracks ([62a7da0](https://github.com/artamrj/magpie-fi/commit/62a7da097513f30683ffe9b6d830323aebc98861))
- **track-inspector:** Enhance track display with chosen metadata ([5df9dcb](https://github.com/artamrj/magpie-fi/commit/5df9dcbb70f66792294d1bde730b2e621d7487f4))
- **audiomack:** Integrate Audiomack as a new metadata provider ([dc68043](https://github.com/artamrj/magpie-fi/commit/dc68043710762c728238bf7ceb188d6d201f4d82))
- **songrec:** Integrate SongRec for audio recognition and metadata enrichment ([a38f70c](https://github.com/artamrj/magpie-fi/commit/a38f70c202891ad3a0e88c868c0607ab368d68d5))
- **navahang:** Integrate Navahang as a new metadata provider ([d20b5e1](https://github.com/artamrj/magpie-fi/commit/d20b5e10148172a4e63db147f31d7d16dc1ea761))
- **shazam:** Integrate Shazam as a new metadata provider ([dc1e496](https://github.com/artamrj/magpie-fi/commit/dc1e4962a9b7b7e63a2511b8868d3bed92607181))
- **apply:** Implement duplicate recording handling and output management ([a549ba9](https://github.com/artamrj/magpie-fi/commit/a549ba94bdcebab920888ea9112bda0a8f60606c))
- **db:** Implement daily cache cleanup and media cache limit enforcement ([baaa661](https://github.com/artamrj/magpie-fi/commit/baaa661842cd5d5c0d899806c852251ec2bfd88f))
- Feat(db): add initial database schema and migration support ([a077106](https://github.com/artamrj/magpie-fi/commit/a0771061bcde05a4dd259d0e7f758ea0fe247d92))
- **docker:** Add Docker support with Compose and CI integration ([29d9757](https://github.com/artamrj/magpie-fi/commit/29d97570f55bbcaab6a931ad4c61877877a05a0e))
- **ci:** Add release workflow and security audit schedule ([8c00cb2](https://github.com/artamrj/magpie-fi/commit/8c00cb2d4a184e1c4cab8ce692a77200663dba0e))
- Feat(ci): enhance CI configuration with improved environment settings ([0e16a3d](https://github.com/artamrj/magpie-fi/commit/0e16a3d042233dd87d1e320e209ce1182e3de2bd))
- **docker:** Enhance entrypoint script and update Dockerfile for user permissions ([7467de4](https://github.com/artamrj/magpie-fi/commit/7467de4da67098c9175120b2b3784dd61c1f0235))
- **api:** Enhance local API protection and origin trust validation ([5c7d4b8](https://github.com/artamrj/magpie-fi/commit/5c7d4b8e09dd4de9ccfe4d723e22afc035f1259f))
- **docker:** Add configurable input mode for music directory ([549472f](https://github.com/artamrj/magpie-fi/commit/549472f6ef1cbb4221687908a11c45b6361f4f5d))
- **apply:** Enhance publish_no_clobber to reuse existing outputs ([52ea8fa](https://github.com/artamrj/magpie-fi/commit/52ea8fa128bff59d87999ab60c7a1f544808d7b0))
- **tracks:** Add remove track functionality with API endpoint ([c07ee12](https://github.com/artamrj/magpie-fi/commit/c07ee12c3b616d605b0f14ba26d3c7d9a62f4c44))
- Add automatic cleaning feature with configurable interval ([cc6618a](https://github.com/artamrj/magpie-fi/commit/cc6618a177096f6ad925493ea61168cd08b8feb4))
- **activity:** Implement frontend activity tracking and automation control ([835252b](https://github.com/artamrj/magpie-fi/commit/835252b8ac666402da6398a21ddd4be34bf7da5e))
- **reference-library:** Add support for read-only reference libraries ([c2883a6](https://github.com/artamrj/magpie-fi/commit/c2883a6f0988e46ec81db951bb0482d5486b0d2d))
- **docker:** Add /data/reference directory for read-only volume mount ([eac2063](https://github.com/artamrj/magpie-fi/commit/eac206369c7ed7dc34e7c92f399cdbd2f7fb7824))
- **docker:** Enhance cargo-chef integration for optimized builds ([ac97e26](https://github.com/artamrj/magpie-fi/commit/ac97e263391398d3e71bbfba08fa922efdc34ee9))
- **reference-library:** Implement duplicate removal after verification ([d5583e0](https://github.com/artamrj/magpie-fi/commit/d5583e055101519ee22d868ddb34978bc2e52195))
- **metadata:** Normalize release fields for candidates in processing ([841128c](https://github.com/artamrj/magpie-fi/commit/841128ce610ba53ecd7ebad21301065fcdc46c11))
- Add structured artist credits and artwork verification workflow ([5881791](https://github.com/artamrj/magpie-fi/commit/58817914da5944d6f132ac632a56a88ef0a9ca0e))
- Add retryable cover download workflow and canonical artist credits ([4b5d7c6](https://github.com/artamrj/magpie-fi/commit/4b5d7c61e64afc27e38a7a16805d345189b2d663))
- Integrate TanStack Query for data fetching and state management ([bc24722](https://github.com/artamrj/magpie-fi/commit/bc2472200a4a5b696f4bc4210f4a37b3e85b1786))
- Add cover art search and manual cover URL support ([ddaea85](https://github.com/artamrj/magpie-fi/commit/ddaea85ef695e265d77287f6502655933856d0ab))
- Initialize sync-me-maybe Telegram bot for music downloads ([ec524aa](https://github.com/artamrj/magpie-fi/commit/ec524aaf30e605be4bf56d9a0ec07473d4c95da5))
- Add support for collections and improve link classification ([358fa7e](https://github.com/artamrj/magpie-fi/commit/358fa7e09bf796ef85095a8a6963e7a65ca41757))
- Update environment configuration and CI workflow for improved deployment ([362bf19](https://github.com/artamrj/magpie-fi/commit/362bf19f6f0157fefd8a3fa0e8ab6e7d9c8ee5ad))
- Add upload batch window configuration and validation ([9cfa15f](https://github.com/artamrj/magpie-fi/commit/9cfa15f13b67c36b067eea52387a46fb01c7e4df))
- Add validation for environment variables in docker-compose ([ec9f528](https://github.com/artamrj/magpie-fi/commit/ec9f5289b88345c7af9a7dcfa1292f5e6367bd34))
- Update environment variable defaults in docker-compose for improved configuration ([f0c7eff](https://github.com/artamrj/magpie-fi/commit/f0c7eff03ca411ddb65e2af36669c5306687b5a8))
- Add comprehensive architecture documentation for project overview ([7f8f210](https://github.com/artamrj/magpie-fi/commit/7f8f2102b357e2d411af421f4b9b11f92dd8d204))
- Implement retry mechanism for job processing with backoff strategy ([9f00e3d](https://github.com/artamrj/magpie-fi/commit/9f00e3d78e0701bab118e700eb95b380ef773018))
- Add Docker support with CI/CD workflows and environment configuration ([bd378b0](https://github.com/artamrj/magpie-fi/commit/bd378b0dba87a987974a8a281f155c7920434395))
- Enhance CI/CD workflow with a release pipeline and remove old container workflow ([f01c04d](https://github.com/artamrj/magpie-fi/commit/f01c04d4fd43a39c2ee3bb6a9693641f8dc4d010))
- Implement upload request stage management and enhance related tests ([2591c0b](https://github.com/artamrj/magpie-fi/commit/2591c0b411cb8d06aa8ed1abaaab5f2aed4a8de7))
- Implement link batching for concurrent message handling in Telegram bot ([3aa83dc](https://github.com/artamrj/magpie-fi/commit/3aa83dc6b08d490779e0aa0f41b8a1d3dfbbbd72))
- Increase MAX_COLLECTION_TRACKS to 1000 and update related documentation ([e5171a6](https://github.com/artamrj/magpie-fi/commit/e5171a632af6447ef23dfc842e0714a06b0f64fc))
- Add changelog for release history and update version to 0.9.0 ([0032e59](https://github.com/artamrj/magpie-fi/commit/0032e598abe9de288dad0fa64012d843151a939d))
- Bump version to 0.9.1 and update changelog for bugfix release ([5da7945](https://github.com/artamrj/magpie-fi/commit/5da7945498f95e2a73525fb7c931c04b2a0f99f8))
- Update changelog and version to 0.9.5; add collection metadata support for downloads ([bfa0c09](https://github.com/artamrj/magpie-fi/commit/bfa0c09886fab13764b04853e80aaeddfa796d20))
- Remove multi-item results button and related callback handling; update changelog for version 0.9.5 ([0b6fc37](https://github.com/artamrj/magpie-fi/commit/0b6fc3743aadcd6ad605275db069ba758fab2490))
- Add skipped/failed details button and related handling; update changelog for version 0.9.6 ([35af4d1](https://github.com/artamrj/magpie-fi/commit/35af4d1eff30c2991e1af64ad31c85b1c57a207b))
- Add AGENTS.md with guidelines for code changes, documentation, verification, and safety ([f81ed03](https://github.com/artamrj/magpie-fi/commit/f81ed03f1d22b4cc91124375cfec62947126f6a1))
- Expand versioning guidelines in AGENTS.md for clarity on version bumps and documentation ([b404f1f](https://github.com/artamrj/magpie-fi/commit/b404f1ffa2ece5fc113c1a4dfe8246bda1ee0f4a))
- Add 'Rerun failed' button for queuing failed jobs in Telegram requests ([7f9fbb8](https://github.com/artamrj/magpie-fi/commit/7f9fbb8c5e3cf2bdb6905fa75e7de30c4dfb16c3))
- Enhance Telegram progress messages with collection details and clearer counters ([9b012e7](https://github.com/artamrj/magpie-fi/commit/9b012e700780dc9447827fdf2c232bfa6e1d6c3e))
- Enhance Telegram message rendering with source labels and improved metadata handling ([d523d70](https://github.com/artamrj/magpie-fi/commit/d523d7060cbba4c54e0f300330a2030783ea73d7))
- Add Telegram status lifecycle with optional received sticker ([9d0eaf0](https://github.com/artamrj/magpie-fi/commit/9d0eaf092cf514ad9a8199c0c2ba36dbc8e051ff))
- Update version to 0.10.1 and enhance Telegram status handling with immediate sticker support ([7de8af3](https://github.com/artamrj/magpie-fi/commit/7de8af338f777f17c01286adee75470e71f78e4a))
- Bump version to 0.10.2 and improve received sticker handling as temporary acknowledgement ([98a1787](https://github.com/artamrj/magpie-fi/commit/98a1787cf7e1d300cec179860cf82c72b3742af7))
- Bump version to 0.10.3 and improve received sticker handling with non-blocking delays ([0963d09](https://github.com/artamrj/magpie-fi/commit/0963d0973324d90c413fa0a6e868e1470295b192))
- Release version 0.10.4 with Spotify playlist pagination and various improvements ([d908f48](https://github.com/artamrj/magpie-fi/commit/d908f480c44beec8d69e2e582c0b4c1b4c9faed8))
- Release version 0.10.5 with single-track metadata support and various improvements ([f1d1da2](https://github.com/artamrj/magpie-fi/commit/f1d1da25789ae4772b238cdb6982b3b10f874f3f))
- **guests:** Add one-time guest access links for music submissions ([ba41529](https://github.com/artamrj/magpie-fi/commit/ba4152988d93dd020fbfafc1bd2f063d5fae6ba1))
- Feat(telegram_bot): index uploaded files and trigger library scan after upload ([8bbee27](https://github.com/artamrj/magpie-fi/commit/8bbee276c2ffb6d2adbe5940f524e88cef8cf7b6))
- Add initial setup for magpie-fi including bot, core, and navidrome integration ([4ae0ac4](https://github.com/artamrj/magpie-fi/commit/4ae0ac40fa5982d85345e6610078ab94aaca61cd))
- Phase 1 - turn ununknown into the magpie program with a real library ([851b118](https://github.com/artamrj/magpie-fi/commit/851b1188c88b1f04efc264fe1f064aabc484752f))
- Phase 2 - get music from links in Rust ([cd3a2d5](https://github.com/artamrj/magpie-fi/commit/cd3a2d5a3d137b1c3483a0baf697c10aae9dbdf9))
- Phase 3 - Telegram bot in Rust, one image ([c27edc5](https://github.com/artamrj/magpie-fi/commit/c27edc595798b3c6bc1533dcc325298f61213289))
- Phase 4 - web page as Telegram Mini App with login ([6e17af9](https://github.com/artamrj/magpie-fi/commit/6e17af939e76906ff54a6a56d1ed3b1339bfc427))
- **TrackRow:** Update artwork display and improve filename handling ([4d8bc92](https://github.com/artamrj/magpie-fi/commit/4d8bc92910a51028c7f87a98b050ef1ddcffee82))
- **dev.sh:** Add development script for backend and frontend setup ([a079d46](https://github.com/artamrj/magpie-fi/commit/a079d46aed8bc3e5fb7764e3445db22eecd76630))

### 🐛 Fixes
- **deps:** Block TypeScript 7 updates ([4dd0561](https://github.com/artamrj/magpie-fi/commit/4dd0561bf80b72e371d510608c158449ab03c8c8))
- Allow explicit user choices to proceed without verified cover ([13aa86c](https://github.com/artamrj/magpie-fi/commit/13aa86cdec626f9f704515e563de3738e15f7b7b))
- Remove page size limit from track loading ([172c2c0](https://github.com/artamrj/magpie-fi/commit/172c2c000b74d1e0809cc1a2273a375dcd1d53a0))
- Standardize backend address to 127.0.0.1 across scripts and config ([7e63f2f](https://github.com/artamrj/magpie-fi/commit/7e63f2fc7ad57c1a84122191c77574b47a7e11f1))
- Correct punctuation in project description for clarity ([561b178](https://github.com/artamrj/magpie-fi/commit/561b1781cd6e835fdcc75c728a069196c3471e5e))
- Update .gitignore to include sync-me-maybe directory ([d50d873](https://github.com/artamrj/magpie-fi/commit/d50d87318ebd2cae18b8cf09936273178debff6a))
- Update CI workflow name and upgrade setup-uv action version ([4e579d4](https://github.com/artamrj/magpie-fi/commit/4e579d4a986b469af546c78e04cd81d47b948088))
- Update environment variable documentation and adjust user/group settings for NAS compatibility ([896e35e](https://github.com/artamrj/magpie-fi/commit/896e35e0649f5854b94d9bb46c5c43eb14d43279))
- Reorder import statements for consistency in callback handling ([a93d5ca](https://github.com/artamrj/magpie-fi/commit/a93d5ca768f08928f3ae3aeffc45e03306809038))
- Improve chat ID retrieval in callback handling for detail messages ([ad19423](https://github.com/artamrj/magpie-fi/commit/ad194236d7525271a5d8ed660805f0afdcbff98f))
- Enhance Spotify collection handling with fallback to embed metadata ([f76034b](https://github.com/artamrj/magpie-fi/commit/f76034baf53c6bacb0e7efb021a64c88e14ba0f5))
- Improve collection folder naming and metadata handling for downloads ([18f56f4](https://github.com/artamrj/magpie-fi/commit/18f56f45f567d7a9e1735fbf96f1dc9ae2151400))
- Set PTB_TIMEDELTA environment variable to avoid deprecation warnings ([7c11461](https://github.com/artamrj/magpie-fi/commit/7c1146114f86fbd439ff1cb28a0edff75376c804))

### 📖 Docs
- Add technical overview and end-to-end algorithm documentation ([de72226](https://github.com/artamrj/magpie-fi/commit/de7222677ae73b87b9bdf1b0bc10f0a320fcfbd9))
- Update README to reflect project goals and phase details ([4c835b8](https://github.com/artamrj/magpie-fi/commit/4c835b86559bc47c8ce2c4d614ae2a4fc5c72cea))

### 📦 Updated tools and libraries
- **deps:** Bump actions/cache from 5 to 6 in the github-actions group ([7544735](https://github.com/artamrj/magpie-fi/commit/754473569ebe3f0832d34d6fba6f04c8a647cd7a))
- **deps:** Bump sqlx in the rust-dependencies group across 1 directory ([2764541](https://github.com/artamrj/magpie-fi/commit/27645410dab7de50df425dc2c3ff106860f75953))
- **deps:** Bump the frontend-dependencies group ([7338f71](https://github.com/artamrj/magpie-fi/commit/7338f71d2ab525d287ad18fbeb1d917e070aa601))
- **deps:** Bump the rust-dependencies group with 3 updates ([e384777](https://github.com/artamrj/magpie-fi/commit/e384777cecc3b54f83b18542c8906a31b265b5b2))
- **deps:** Bump the rust-dependencies group across 1 directory with 8 updates ([d20bb0a](https://github.com/artamrj/magpie-fi/commit/d20bb0a5394aeb071f18990ebd9535927271cf83))
- **deps:** Bump rust in the docker-images group ([a5f7961](https://github.com/artamrj/magpie-fi/commit/a5f796181393314df1b9055706e54a1a55d14cb0))
- **deps:** Bump the frontend-dependencies group across 1 directory with 10 updates ([5b77285](https://github.com/artamrj/magpie-fi/commit/5b77285e88c72b1808ace446ece5b902388d4152))

### 🔧 Behind the scenes
- Init ([b745f71](https://github.com/artamrj/magpie-fi/commit/b745f715dc480ada9b3f53caaf5b7d2d7c10ded7))
- Initial commit ([ad73a93](https://github.com/artamrj/magpie-fi/commit/ad73a93d3fb20cff560156f39a61b1297a1a6d70))
- Refactor project structure and implement HTTP routing ([99febec](https://github.com/artamrj/magpie-fi/commit/99febec5eb3e5b7eea22587251ee629ef1478e73))
- Remove unused Output column from PreviewVirtualList component ([552d13f](https://github.com/artamrj/magpie-fi/commit/552d13f5c218fcf33f6487860475a8b195abc1b1))
- Remove obsolete Docker and CI configuration files ([dd78d80](https://github.com/artamrj/magpie-fi/commit/dd78d80125bdf4b2d8c5b3f3eaa97d12aa9ee997))
- Implement scan and job management endpoints ([61d434a](https://github.com/artamrj/magpie-fi/commit/61d434a74c9391abf02378c4bbb03b98f642c126))
- Refactor workflow management in AppState and related handlers ([b1e2ec7](https://github.com/artamrj/magpie-fi/commit/b1e2ec7ca9d94ea329535b058c2a850e4bd26e20))
- Refactor terminal logging to activity logging ([38ca7c3](https://github.com/artamrj/magpie-fi/commit/38ca7c3d708010da48614dfb7d8c2f99acd4295c))
- Refactor(api): simplify workspace handling and remove unused routes ([50205aa](https://github.com/artamrj/magpie-fi/commit/50205aa26d05de991d04d6af81c37f6346b38432))
- REDEASIGN FRONTEND -- remove redundant code blocks from changes section ([bb24029](https://github.com/artamrj/magpie-fi/commit/bb2402933688a9c8ab1766649f7ee2ff0d06d57b))
- Remove redundant code blocks from changes section ([d50d5d5](https://github.com/artamrj/magpie-fi/commit/d50d5d528bdd8258d51de3a240b9160e7c4ec4ca))
- Simplify CI configuration and improve server health check logic ([d254ea4](https://github.com/artamrj/magpie-fi/commit/d254ea422df22d5c0b0f879645f4ccf341f1d7f2))
- Upgrade dependencies and update configuration files ([50ef47a](https://github.com/artamrj/magpie-fi/commit/50ef47ac39112eddb6c969804d0c8c1659aa7215))
- Fix verify failure by auditing dynamic sqlx queries ([176a661](https://github.com/artamrj/magpie-fi/commit/176a661dd0771bd32641fdc379b1ef3de2e5ee6a))
- **deps-dev:** Bump eslint ([33efd4b](https://github.com/artamrj/magpie-fi/commit/33efd4bc1056142f95df899f03a7f9304981e1ad))
- Initial plan ([53b30ec](https://github.com/artamrj/magpie-fi/commit/53b30eccde285ca8e486e4f73fcf80623843412a))
- Fix Trivy platform for arm64 release scan ([b6a9eb2](https://github.com/artamrj/magpie-fi/commit/b6a9eb287c92d8df653663968cb9199e7d640397))
- Refactor[Big Chnage Duplicate Check]!: remove reference library feature and deduplicate within input only ([5381287](https://github.com/artamrj/magpie-fi/commit/5381287bd08cbf3327d186cfe4f943a05225259f))
- Centralize workflow claiming and phase transitions ([6911f96](https://github.com/artamrj/magpie-fi/commit/6911f96321beee59c9145277c041b7578821fda8))
- Rename scan_pipeline module to scan ([c39af4c](https://github.com/artamrj/magpie-fi/commit/c39af4caa7267095b14bb3dc26bd7d1543cfec89))
- Consolidate scoring logic into domain::matcher ([cbed569](https://github.com/artamrj/magpie-fi/commit/cbed569bdb13ae160a8e527506bf66f2e5c6f934))
- Consolidate track and candidate types into infrastructure::db::tracks ([56af13d](https://github.com/artamrj/magpie-fi/commit/56af13d4b55f7c6b3d008a5cb8ed07c930e60e5f))
- Consolidate track types and status into domain-specific modules ([6de7118](https://github.com/artamrj/magpie-fi/commit/6de7118e63d50325d375308942b2692da58a8db0))
- Reorganize scan pipeline into modular components ([040dee8](https://github.com/artamrj/magpie-fi/commit/040dee86a74ebc2abe18a532e3a39e7e56f61ea7))
- Import ununknown history into core/ ([bcce514](https://github.com/artamrj/magpie-fi/commit/bcce5145be72c16fb51cd67e313f754ded3729da))
- Initial commit ([819a3bf](https://github.com/artamrj/magpie-fi/commit/819a3bf50cd50837e83d01f1047e1afbc4f5df4d))
- Remove deprecated files and update environment variable handling for local paths ([9738750](https://github.com/artamrj/magpie-fi/commit/97387506a715d6e4b8d5905d586ca07307264655))
- Remove unused test files and update installation instructions ([12a4ca0](https://github.com/artamrj/magpie-fi/commit/12a4ca00a7de2e8659aa4f4f9388ad799c7cee19))
- Refactor code structure for improved readability and maintainability ([0969499](https://github.com/artamrj/magpie-fi/commit/0969499359faac57a500556ac986b939fb6f7148))
- Implement music provider architecture with support for YouTube, Spotify, Apple Music, and Shazam ([4f904de](https://github.com/artamrj/magpie-fi/commit/4f904de884464424e2972cae4414c1f07e1cb581))
- Enhance documentation and improve code clarity across multiple modules ([87882a9](https://github.com/artamrj/magpie-fi/commit/87882a927e9c401b860e25534502c7c4c8f1c9c3))
- Refactor tests: Remove old retry tests and add new queue and telegram tests ([eef3873](https://github.com/artamrj/magpie-fi/commit/eef3873766640730500682392120b4901c74ce2f))
- Simplify track destination filename structure and update related tests ([97b50a4](https://github.com/artamrj/magpie-fi/commit/97b50a47bfa0784170b916cf31bef3b53bcf5f34))
- Streamline error handling and assertions in link processing and test cases ([698877b](https://github.com/artamrj/magpie-fi/commit/698877b50ca74daceb20a847226a77480ab2f5be))
- Update changelog and bump version to 0.9.6 with new features and improvements ([e448a60](https://github.com/artamrj/magpie-fi/commit/e448a60ae1de15bfed52266dd493e6c8cb605160))
- Improve assertion readability in process link job test ([f05ef41](https://github.com/artamrj/magpie-fi/commit/f05ef4195ef7e29d7cf11c71c747c5e99d477fa4))
- Reduce sleep duration in received sticker test to improve performance ([38d6c00](https://github.com/artamrj/magpie-fi/commit/38d6c00c89403da58fa1bc9100abb2fc06a0aef8))
- Update .gitignore to include .DS_Store ([e1264ac](https://github.com/artamrj/magpie-fi/commit/e1264ac9e95d4aa9f4e260ba4a39eec807a48d98))
- Remove empty code change entries from the changelog ([b0eb1cb](https://github.com/artamrj/magpie-fi/commit/b0eb1cb9f5f3560710b281518dbb511cb3842207))
- Import sync-me-maybe history into bot/ ([cba2382](https://github.com/artamrj/magpie-fi/commit/cba2382810ea269c7faab4255ac5a83923eaf97b))
- Remove support for self-hosted Telegram Bot API server and clarify file limits ([94829e5](https://github.com/artamrj/magpie-fi/commit/94829e51db34b0b4b62b011be08f9d22408fe4c1))
- Remove Navidrome login requirements and related configurations ([e52cf8c](https://github.com/artamrj/magpie-fi/commit/e52cf8ca325ffcde65adb3298e01dfe6b9f8e724))
- Remove rhyme feature and related configurations from the bot ([ae702e9](https://github.com/artamrj/magpie-fi/commit/ae702e9086fc3811f26fecfbcbda0a5b7c5499e9))
- Remove SECURITY.md and add detailed blueprint documentation ([e656d56](https://github.com/artamrj/magpie-fi/commit/e656d56ebe6e68ce659d0d216a677e3f8b89e42b))
- Use the magpie-fi name everywhere ([724f47d](https://github.com/artamrj/magpie-fi/commit/724f47d40f4da0261ddfbfe4b4b2cba82f2963fd))
