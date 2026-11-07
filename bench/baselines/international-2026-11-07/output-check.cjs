const fs=require('node:fs');const vm=require('node:vm');const {isDeepStrictEqual}=require('node:util');const {createHash}=require('node:crypto');
const addon=process.env.NAPI_BENCH_BINDING;const {Vm}=require(addon);const owner=new Vm();
const cases={
 number_parts:'new Intl.NumberFormat("en-US").formatToParts(1234.5)',
 date_parts:'new Intl.DateTimeFormat("en-US",{timeZone:"UTC",year:"numeric",month:"2-digit",day:"2-digit"}).formatToParts(1704067200000)',
 duration_parts:'new Intl.DurationFormat("en-US",{style:"long"}).formatToParts({hours:1,minutes:2})',
 segment_words:'Array.from(new Intl.Segmenter("en-US",{granularity:"word"}).segment("one two"))',
 display_names:'new Intl.DisplayNames("en-US",{type:"region"}).of("US")',
 temporal_date_add:'(()=>{var start=Temporal.PlainDate.from("2024-01-01"),d=start;for(var i=0;i<100;i++)d=d.add({days:1});return {date:d.toString(),days:d.since(start).days};})()',
 temporal_datetime_add:'(()=>{var d=Temporal.PlainDateTime.from("2024-01-01T00:00");for(var i=0;i<100;i++)d=d.add({minutes:1});return d.toString();})()',
 temporal_instant_round:'Temporal.Instant.from("2024-01-01T00:00:00.123456789Z").round({smallestUnit:"millisecond",roundingMode:"halfExpand"}).toString()',
 temporal_zoned_utc:'Temporal.ZonedDateTime.from("2024-02-29T12:34:56+00:00[UTC]").toString()',
 temporal_duration_round:'Temporal.Duration.from("PT90M").round({largestUnit:"hour",smallestUnit:"minute"}).toString()',
};
const sha=x=>createHash('sha256').update(x).digest('hex');const results=[];
try{for(const [name,expr]of Object.entries(cases)){const source='JSON.stringify('+expr+');';const native=JSON.parse(owner.run(source));const node=JSON.parse(vm.runInNewContext(source));results.push({name,source,source_sha256:sha(source),native,node,equal:isDeepStrictEqual(native,node)});}}finally{owner.dispose();}
console.log(JSON.stringify({schema:1,node:process.version,v8:process.versions.v8,icu:process.versions.icu,addon_sha256:sha(fs.readFileSync(addon)),script_sha256:sha(fs.readFileSync(__filename)),results},null,2));
